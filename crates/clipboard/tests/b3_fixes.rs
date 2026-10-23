//! B3 fixes (the Astra B3 review), clipboard side: the native envelope is
//! refused when its bytes are decoded, before any paste (findings 3 and 4).
use std::collections::BTreeMap;

use reprise_clipboard::{ClipboardError, NativeFragment};
use reprise_doc::fragment::Fragment;
use reprise_doc::{BlockKind, Document, SchemaRegistry};
use serde_json::{Value, json};

fn base() -> Fragment {
    let d = Document::new(1).unwrap();
    d.append_block(BlockKind::Paragraph, "", "body").unwrap();
    d.commit();
    d.copy_all_fragment("source", &SchemaRegistry::builtin())
        .unwrap()
}

fn story_blocks(f: &Fragment, n: usize) -> Vec<Value> {
    (0..n)
        .map(|i| {
            let mut b = serde_json::to_value(&f.blocks[0]).unwrap();
            b["id"] = json!(format!("{}@99", i + 1));
            b["text"] = json!("story");
            b
        })
        .collect()
}

fn native(fragment: Fragment) -> NativeFragment {
    NativeFragment {
        fragment,
        resources: BTreeMap::new(),
        notes: Vec::new(),
    }
}

fn with_stories(mut f: Fragment, stories: Value) -> Fragment {
    f.sections = Some(json!({"v": 1, "mode": "complete", "stories": stories}).to_string());
    f
}

#[test]
fn a_story_block_parented_outside_its_story_is_refused_on_decode() {
    let f = base();
    let body = serde_json::to_value(f.blocks[0].id).unwrap();
    let mut into_body = story_blocks(&f, 1);
    into_body[0]["parent"] = body;
    let mut into_story = story_blocks(&f, 2);
    into_story[1]["parent"] = json!("1@99");
    for (what, fragment) in [
        (
            "into the body",
            with_stories(
                f.clone(),
                json!([{"label": "s0", "source": "1@8", "blocks": into_body}]),
            ),
        ),
        (
            "into another story",
            with_stories(
                f.clone(),
                json!([
                    {"label": "s0", "source": "1@8", "blocks": [story_blocks(&f, 2)[0].clone()]},
                    {"label": "s1", "source": "2@8", "blocks": [into_story[1].clone()]},
                ]),
            ),
        ),
    ] {
        let n = native(fragment);
        assert!(
            matches!(n.validate(), Err(ClipboardError::Fragment(_))),
            "{what}: {:?}",
            n.validate()
        );
        assert!(n.encode().is_err(), "{what}");
        let bytes = serde_json::to_vec(&n).unwrap();
        assert!(NativeFragment::decode(&bytes).is_err(), "{what}");
    }
}

#[test]
fn an_over_limit_story_nothing_binds_is_refused_on_decode() {
    let f = base();
    let big = with_stories(
        f.clone(),
        json!([{"label": "s0", "source": "1@8", "blocks": story_blocks(&f, 4097)}]),
    );
    let n = native(big);
    assert!(n.validate().is_err());
    assert!(NativeFragment::decode(&serde_json::to_vec(&n).unwrap()).is_err());
    // Four thousand and ninety-six blocks are within the limit.
    let ok = with_stories(
        f.clone(),
        json!([{"label": "s0", "source": "1@8", "blocks": story_blocks(&f, 4095)}]),
    );
    native(ok).validate().unwrap();
}
