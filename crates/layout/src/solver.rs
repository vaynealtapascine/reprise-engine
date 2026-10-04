//! Integer allocation only inside an explicitly declared domain (25).
use crate::codes;
use reprise_diag::{Note, Severity};
use reprise_geom::{Length, div_round};

pub const MAX_DOMAIN_VARIABLES: usize = 256;
pub const MAX_SOLVER_ITERATIONS: usize = 256;

/// A nonnegative width, its closed bounds and its share of surplus width.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Variable {
    pub min: Length,
    pub max: Length,
    pub weight: u32,
}

/// Declaration is required: there is no document-wide constraint solver.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SolverDomain {
    pub budget: Length,
    pub variables: Vec<Variable>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Solution {
    pub widths: Vec<Length>,
    pub notes: Vec<Note>,
}

impl SolverDomain {
    /// Bounded water filling. Infeasible bounds fall back to equal columns;
    /// underspecified surplus is shared equally between non-fixed variables.
    pub fn solve(&self) -> Solution {
        let mut notes = Vec::new();
        let vars = &self.variables;
        let n = vars.len().min(MAX_DOMAIN_VARIABLES);
        if vars.len() > MAX_DOMAIN_VARIABLES {
            notes.push(Note::new(
                Severity::Error,
                codes::SOLVER_LIMIT,
                "domain exceeds 256 variables; extra variables omitted",
            ));
        }
        let budget = i64::from(self.budget.0.max(0));
        let valid = self.budget >= Length::ZERO
            && n > 0
            && vars
                .iter()
                .take(n)
                .all(|v| v.min >= Length::ZERO && v.max >= v.min);
        let minimum: i64 = vars.iter().take(n).map(|v| i64::from(v.min.0.max(0))).sum();
        if !valid || minimum > budget {
            notes.push(Note::new(
                Severity::Warning,
                codes::SOLVER_INFEASIBLE,
                "infeasible domain; bounds relaxed to equal columns",
            ));
            return Solution {
                widths: equal(budget, n),
                notes,
            };
        }
        let mut widths: Vec<i64> = vars.iter().take(n).map(|v| i64::from(v.min.0)).collect();
        let mut remaining = budget - minimum;
        let mut under = false;
        for _ in 0..MAX_SOLVER_ITERATIONS {
            if remaining == 0 {
                break;
            }
            let active: Vec<usize> = (0..n)
                .filter(|&i| {
                    widths.get(i).copied().unwrap_or(0)
                        < i64::from(vars.get(i).map_or(Length::ZERO, |v| v.max).0)
                })
                .collect();
            if active.is_empty() {
                under = true;
                break;
            }
            let weight: i64 = active
                .iter()
                .filter_map(|&i| vars.get(i))
                .map(|v| i64::from(v.weight))
                .sum();
            let equal_weight = weight == 0;
            under |= equal_weight;
            let denominator = if equal_weight {
                active.len() as i64
            } else {
                weight
            };
            let before = remaining;
            for &i in &active {
                let (Some(v), Some(w)) = (vars.get(i), widths.get_mut(i)) else {
                    continue;
                };
                let share = if equal_weight { 1 } else { i64::from(v.weight) };
                let amount = div_round(before.saturating_mul(share), denominator)
                    .min(i64::from(v.max.0) - *w)
                    .min(remaining)
                    .max(0);
                *w += amount;
                remaining -= amount;
            }
            if remaining == before {
                // Rounding left fewer subunits than active shares. A stable
                // document-order residual assignment guarantees progress.
                for &i in &active {
                    if remaining == 0 {
                        break;
                    }
                    if let Some(w) = widths.get_mut(i) {
                        *w += 1;
                        remaining -= 1;
                    }
                }
            }
        }
        if remaining > 0 && !under {
            notes.push(Note::new(
                Severity::Warning,
                codes::SOLVER_LIMIT,
                "allocation reached its 256-iteration limit; unused width retained",
            ));
        }
        if under {
            notes.push(Note::new(Severity::Warning, codes::SOLVER_UNDERCONSTRAINED, "surplus has no declared share or exceeds upper bounds; equal sharing or unused width used"));
        }
        Solution {
            widths: widths
                .into_iter()
                .map(|w| Length(w.clamp(0, i64::from(i32::MAX)) as i32))
                .collect(),
            notes,
        }
    }
}

fn equal(budget: i64, n: usize) -> Vec<Length> {
    if n == 0 {
        return Vec::new();
    }
    let mut remaining = budget;
    (0..n)
        .map(|i| {
            let width = div_round(remaining, (n - i) as i64).min(remaining);
            remaining -= width;
            Length(width as i32)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn infeasible_and_extreme_domains_never_wrap() {
        for budget in [Length::MIN, Length::ZERO, Length::MAX] {
            let result = SolverDomain {
                budget,
                variables: vec![
                    Variable {
                        min: Length::MAX,
                        max: Length::MIN,
                        weight: u32::MAX
                    };
                    256
                ],
            }
            .solve();
            assert_eq!(result.widths.len(), 256);
            assert!(result.widths.iter().all(|w| *w >= Length::ZERO));
            assert_eq!(
                result.widths.iter().map(|w| i64::from(w.0)).sum::<i64>(),
                i64::from(budget.0.max(0))
            );
        }
        assert!(
            !SolverDomain {
                budget: Length::ZERO,
                variables: vec![]
            }
            .solve()
            .notes
            .is_empty()
        );
    }
    #[test]
    fn capped_weights_redistribute_and_preserve_every_subunit() {
        let result = SolverDomain {
            budget: Length(11),
            variables: vec![
                Variable {
                    min: Length(2),
                    max: Length(3),
                    weight: 10,
                },
                Variable {
                    min: Length(1),
                    max: Length(20),
                    weight: 1,
                },
            ],
        }
        .solve();
        assert_eq!(result.widths, [Length(3), Length(8)]);
        assert!(result.notes.is_empty());
    }
    #[test]
    fn undeclared_shares_and_hostile_sizes_are_reported() {
        let v = Variable {
            min: Length::ZERO,
            max: Length::MAX,
            weight: 0,
        };
        assert_eq!(
            SolverDomain {
                budget: Length(1),
                variables: vec![v; 2]
            }
            .solve()
            .widths,
            [Length(1), Length(0)]
        );
        assert_eq!(
            SolverDomain {
                budget: Length(1),
                variables: vec![v; 10000]
            }
            .solve()
            .widths
            .len(),
            MAX_DOMAIN_VARIABLES
        );
    }
}
