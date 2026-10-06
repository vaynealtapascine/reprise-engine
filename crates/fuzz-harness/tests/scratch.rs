use reprise_fuzz_harness::input::seed_bytes;
use reprise_fuzz_harness::scenario::Op;
use reprise_fuzz_harness::*;
#[test]
fn scratch() {
    let seed: u64 = std::env::var("SEED").unwrap().parse().unwrap();
    let k: usize = std::env::var("K").unwrap().parse().unwrap();
    let mut s = Scenario::decode(&seed_bytes(seed, 300));
    if std::env::var("SHOW").is_ok() {
        println!("{:#?}", s.ops.get(k.saturating_sub(1)));
    }
    s.ops.truncate(k);
    if let Ok(f) = std::env::var("FLAGS") {
        if let Some(Op::Render { flags, .. }) = s.ops.last_mut() {
            *flags = f.parse().unwrap();
        }
    }
    match run_scenario(&s) {
        Ok(r) => println!("OK {:?}", r.counts),
        Err(v) => println!("{}", v.to_string().chars().take(1500).collect::<String>()),
    }
}
