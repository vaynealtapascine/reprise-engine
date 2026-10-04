//! Page templates for tests: columns with a margin beside them, sized
//! absolutely or by the medium.

use reprise_doc::{Basis, Dim, FrameRole, FrameTemplate, MAIN_FLOW, PageTemplate};

/// A main-flow frame.
pub fn flow_frame(name: &str, x: Dim, y: Dim, width: Dim, height: Dim) -> FrameTemplate {
    FrameTemplate::new(
        name,
        FrameRole::Flow(MAIN_FLOW.into()),
        (x, y),
        (width, height),
    )
}

/// A margin frame.
pub fn margin_frame(name: &str, x: Dim, y: Dim, width: Dim, height: Dim) -> FrameTemplate {
    FrameTemplate::new(name, FrameRole::Margin, (x, y), (width, height))
}

/// 420×300 pt with two 130 pt columns that are 80 pt deep (six lines of the
/// body style), and a 100 pt margin frame, all in absolute lengths. The main
/// flow goes down `left`, then `right`, then on to the next page.
pub fn two_columns() -> PageTemplate {
    PageTemplate::new("two-columns", Dim::pt(420), Dim::pt(300))
        .with_frame(flow_frame(
            "left",
            Dim::pt(24),
            Dim::pt(30),
            Dim::pt(130),
            Dim::pt(80),
        ))
        .with_frame(flow_frame(
            "right",
            Dim::pt(164),
            Dim::pt(30),
            Dim::pt(130),
            Dim::pt(80),
        ))
        .with_frame(margin_frame(
            "margin",
            Dim::pt(304),
            Dim::pt(30),
            Dim::pt(100),
            Dim::pt(80),
        ))
}

/// A page as large as the medium, with two columns that are 45% of the page
/// wide and a margin frame, all measured from the page and the medium. It
/// reflows when the medium changes.
pub fn responsive_columns() -> PageTemplate {
    let of_width = |permille| Dim::fraction(Basis::PageWidth, permille);
    let depth = Dim::fraction(Basis::PageHeight, 1000).plus(reprise_geom::Length::from_pt(-60));
    PageTemplate::new(
        "responsive",
        Dim::fraction(Basis::MediumWidth, 1000),
        Dim::fraction(Basis::MediumHeight, 1000),
    )
    .with_frame(flow_frame(
        "left",
        Dim::pt(20),
        Dim::pt(30),
        of_width(300),
        depth,
    ))
    .with_frame(flow_frame(
        "right",
        of_width(350).plus(reprise_geom::Length::from_pt(10)),
        Dim::pt(30),
        of_width(300),
        depth,
    ))
    .with_frame(margin_frame(
        "margin",
        of_width(700).plus(reprise_geom::Length::from_pt(30)),
        Dim::pt(30),
        of_width(250).plus(reprise_geom::Length::from_pt(-40)),
        depth,
    ))
}

/// A long passage of plain text, `repeat` times.
pub fn long_text(repeat: usize) -> String {
    [crate::spike::OPENING, crate::spike::HALLWAY]
        .iter()
        .cycle()
        .take(repeat.saturating_mul(2))
        .copied()
        .collect::<Vec<_>>()
        .join(" ")
}
