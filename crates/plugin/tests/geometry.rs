mod common;
use common::{bytes, load};
use reprise_compose::{Available, GeometryProvider, Interval, LineQuery, Measure};
use reprise_geom::Length;
use reprise_plugin::PluginGeometry;

fn record(tag: i32, n: i32, words: &[i32]) -> String {
    let mut body = String::new();
    for (i, word) in [tag, n].iter().chain(words).enumerate() {
        body.push_str(&format!(
            "local.get $out i32.const {word} i32.store offset={} ",
            i * 4
        ));
    }
    body.push_str(&format!("i32.const {}", 8 + words.len() * 4));
    body
}

#[test]
fn malformed_shapes_fall_back_to_the_exact_frame_measure() {
    let measure = Measure(Length(100));
    let query = LineQuery {
        line: 0,
        block_offset: Length::ZERO,
        line_height: Length(10),
        previous: &[],
    };
    for body in [
        record(0, 1, &[90, 20]),
        record(0, 1, &[-1, 100]),
        record(0, 1, &[0, 101]),
        record(0, 1, &[0, 0]),
        record(0, 2, &[0, 60, 50, 100]),
        record(0, 65, &[]),
        record(1, 0, &[]),
        record(2, 1, &[]),
        "unreachable".into(),
        "(loop $forever br $forever) i32.const 0".into(),
    ] {
        let plugin = load(&bytes(&body, ""));
        let geometry = PluginGeometry::new(Some(&plugin), 0, &measure);
        for _ in 0..2 {
            assert_eq!(geometry.available(&query), measure.available(&query));
        }
        assert_eq!(geometry.notes().len(), 1);
        assert!(matches!(
            geometry.notes()[0].code.as_str(),
            "plugin.result" | "plugin.trap" | "plugin.fuel"
        ));
    }
    let missing = PluginGeometry::new(None, 0, &measure);
    assert_eq!(missing.available(&query), measure.available(&query));
    assert_eq!(missing.notes()[0].code, "plugin.unavailable");
}

#[test]
fn valid_room_skip_end_and_extreme_queries_are_total() {
    let measure = Measure(Length::MAX);
    for (body, expected) in [
        (
            record(0, 2, &[0, 10, 20, 100]),
            Available::Room(vec![
                Interval::new(Length(0), Length(10)),
                Interval::new(Length(20), Length(100)),
            ]),
        ),
        (record(1, 100, &[]), Available::Skip { next: Length(100) }),
        (record(2, 0, &[]), Available::End),
    ] {
        let plugin = load(&bytes(&body, ""));
        let geometry = PluginGeometry::new(Some(&plugin), 0, &measure);
        let query = LineQuery {
            line: u32::MAX,
            block_offset: Length::MIN,
            line_height: Length::MAX,
            previous: &[],
        };
        assert_eq!(geometry.available(&query), expected);
        assert!(geometry.notes().is_empty());
    }
    for width in [Length::MIN, Length(-1), Length::ZERO] {
        let measure = Measure(width);
        let plugin = load(&bytes(&record(0, 1, &[0, 10]), ""));
        let geometry = PluginGeometry::new(Some(&plugin), 0, &measure);
        let query = LineQuery {
            line: 0,
            block_offset: Length::MAX,
            line_height: Length::MIN,
            previous: &[],
        };
        assert_eq!(geometry.available(&query), measure.available(&query));
    }
}

#[test]
fn shapes_cannot_bridge_existing_exclusions() {
    struct Split;
    impl GeometryProvider for Split {
        fn available(&self, _: &LineQuery<'_>) -> Available {
            Available::Room(vec![
                Interval::new(Length(0), Length(10)),
                Interval::new(Length(20), Length(30)),
            ])
        }
    }
    let plugin = load(&bytes(&record(0, 1, &[0, 30]), ""));
    let geometry = PluginGeometry::new(Some(&plugin), 0, &Split);
    let query = LineQuery {
        line: 0,
        block_offset: Length::ZERO,
        line_height: Length(10),
        previous: &[],
    };
    assert_eq!(geometry.available(&query), Split.available(&query));
    assert_eq!(geometry.notes()[0].code, "plugin.result");
}
