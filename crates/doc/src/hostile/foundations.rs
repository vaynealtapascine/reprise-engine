//! A hostile peer for the wave-1 stored forms (feature `hostile-peer`).
//!
//! `scribble` knows the roots that existed when it was written. These attack
//! what wave 1 added: the inline record map `inline1`, annotation envelopes
//! (`annot1`) and restore hints on `ranges`, the `lists` tree and the
//! paragraph keys `role1`, `list1`, `level1` and `restart1`, the style roots
//! `styles1`, `para1` and `styles2` with their control-prefixed references,
//! and the document identity roots. Every value is one a peer could write:
//! the right type with a wrong value, the wrong type, near-valid text,
//! extreme numbers, a reference that points at the wrong kind of thing.
//!
//! Tree-creating attacks stay shallow (new nodes go under the root), so these
//! runs never build the 1,024-deep trees that `LoroDoc::get_value` recurses
//! through (Astra 4 R2).

use loro::{Container, LoroMap, LoroTree, LoroValue, TreeID, TreeParentId, ValueOrContainer};

use super::{Rng, nodes, texts, value};
use crate::Document;

const ENVELOPES: &[&str] = &[
    r#"{"version":1,"kind":"link","payload":{"type":"url","url":"https://example.com/x"}}"#,
    r#"{"version":1,"kind":"link","payload":{"type":"url","url":"javascript:alert(1)"}}"#,
    r#"{"version":1,"kind":"link","payload":{"type":"url","url":""}}"#,
    r#"{"version":1,"kind":"link","payload":{"type":"node","node":"0@0"}}"#,
    r#"{"version":1,"kind":"link","payload":{"type":"heading","node":"999@999"}}"#,
    r#"{"version":1,"kind":"link","payload":{"type":"range","range":"0@0"}}"#,
    r#"{"version":1,"kind":"comment","payload":{"thread":"t1"}}"#,
    r#"{"version":1,"kind":"comment","payload":{"thread":"bad thread!"}}"#,
    r#"{"version":1,"kind":"comment","payload":{"thread":""}}"#,
    r#"{"version":2,"kind":"link","payload":{}}"#,
    r#"{"version":0,"kind":"comment","payload":{"thread":"t"}}"#,
    r#"{"version":1,"kind":"future-kind","payload":{"x":1}}"#,
    r#"{"version":1,"kind":"link","payload":{"type":"url","url":"https://a.example/","extra":1}}"#,
    r#"{"version":1,"kind":"link"}"#,
    r#"{"version":1,"kind":7,"payload":null}"#,
    "{",
    "[]",
    "null",
    "",
    "\0",
];

const HINTS: &[&str] = &[
    "[]",
    "[",
    "null",
    r#"[{"version":1,"base":null,"count":4294967295,"digest":"00","peer":"1","after":0,"start":null,"end":null}]"#,
    r#"[{"version":1,"base":"zz","count":1,"digest":"","peer":"x","after":-1,"start":{"anchor":"q","offset":9},"end":null}]"#,
    r#"[{"version":9}]"#,
];

const SIZES: &[&str] = &[
    "pt:36864",
    "pt:0",
    "pt:-1024",
    "pt:9223372036854775807",
    "em:1000",
    "em:20000000000",
    "pct:500",
    "pct:5000000",
    "garbage",
    "",
];

const FORMATS: &[&str] = &[
    "bullet",
    "decimal",
    "decimal-leading-zero",
    "lower-alpha",
    "upper-alpha",
    "lower-roman",
    "upper-roman",
    "none",
    "BOGUS",
    "",
];

const TEMPLATES: &[&str] = &[
    "%1.", "%0", "%9%9%9", "%", "%%%", "\u{2022}", "(%1)", "", "%1.%2.%3",
];

const NUMBERS: &[&str] = &[
    "0",
    "1",
    "-5",
    "9223372036854775807",
    "9223372036854775808",
    "-9223372036854775808",
    "1e3",
    "abc",
    "",
];

const EXPRESSIONS: &[&str] = &[
    "12pt",
    "-9999999999pt",
    "1000000em",
    "calc(1/0)",
    "garbage",
    "",
    "5%",
    "1lh",
    "2147483647pt",
    "0",
];

const PROPERTIES: &[&str] = &[
    "size",
    "line-height",
    "alignment",
    "tabs",
    "weight",
    "slant",
    "indent-start",
    "indent-end",
    "indent-first",
    "space-before",
    "space-after",
    "keep-with-next",
    "keep-together",
    "widows",
    "orphans",
    "background",
    "parent",
    "families",
    "zz-unknown",
];

const STYLE_NAMES: &[&str] = &[
    "",
    "Normal",
    "Heading 1",
    "S0",
    "S1",
    "loop",
    "a/b",
    "\u{1}style1:00",
    "\u{2}builtin1:Normal",
    "\u{3}1:5",
];

fn ids(tree: &LoroTree) -> Vec<String> {
    nodes(tree).iter().map(ToString::to_string).collect()
}

fn pick_str(rng: &mut Rng, items: &[&str]) -> String {
    (*rng.pick(items).unwrap_or(&"")).to_owned()
}

/// Mostly `text`; sometimes another type, or a very long string.
fn scalar_or_text(rng: &mut Rng, text: String) -> LoroValue {
    match rng.below(12) {
        0 => LoroValue::Null,
        1 => LoroValue::Bool(rng.chance(50)),
        2 => LoroValue::I64(
            *rng.pick(&[i64::MIN, -1, 0, 1, 9, 255, i64::MAX])
                .unwrap_or(&0),
        ),
        3 => LoroValue::Double(
            *rng.pick(&[f64::NAN, f64::INFINITY, 3.5, -0.0])
                .unwrap_or(&0.0),
        ),
        4 => LoroValue::Binary(vec![0xde, 0xad, 0xbe, 0xef].into()),
        5 => LoroValue::String("x".repeat(rng.below(40_000)).into()),
        _ => LoroValue::String(text.into()),
    }
}

/// Writes `ops` raw operations aimed at the wave-1 stored forms into `doc`'s
/// store and commits. Like [`super::scribble`], `doc` should be a fork of the
/// victim with its own peer.
pub fn scribble_foundations(doc: &Document, rng: &mut Rng, ops: usize) {
    for _ in 0..ops {
        let _ = foundation_op(doc, rng);
        if rng.chance(20) {
            doc.commit();
        }
    }
    doc.commit();
}

fn foundation_op(doc: &Document, rng: &mut Rng) -> loro::LoroResult<()> {
    match rng.below(12) {
        0..=2 => inline_op(doc, rng),
        3..=4 => annotation_op(doc, rng),
        5..=6 => list_op(doc, rng),
        7..=8 => paragraph_op(doc, rng),
        9 => style_op(doc, rng),
        10 => identity_op(doc, rng),
        _ => marker_op(doc, rng),
    }
}

fn inline_entry_value(rng: &mut Rng, entry: &str) -> LoroValue {
    let text = match entry {
        "kind" => pick_str(rng, &["image", "field", "", "future-kind", "IMAGE"]),
        "align" => pick_str(
            rng,
            &["baseline", "middle", "text-top", "text-bottom", "bogus", ""],
        ),
        "p:asset" => pick_str(rng, &["00", "", &"a".repeat(64), &"A".repeat(64), "zz"]),
        "p:width" | "p:height" => pick_str(rng, SIZES),
        "p:field" => pick_str(
            rng,
            &["page-number", "page-count", "page-of", "text", "bogus", ""],
        ),
        "p:target" => pick_str(rng, &["", "bogus", "copy:0@0 0", "0@0"]),
        _ => pick_str(rng, &["x", "", "\u{fffc}", "\0", "word"]),
    };
    scalar_or_text(rng, text)
}

fn inline_op(doc: &Document, rng: &mut Rng) -> loro::LoroResult<()> {
    let map = doc.inline_map();
    let keys: Vec<String> = map.keys().map(|k| k.to_string()).collect();
    let record = |key: &str| match map.get(key) {
        Some(ValueOrContainer::Container(Container::Map(m))) => Some(m),
        _ => None,
    };
    match rng.below(7) {
        0..=2 => {
            if let Some(rec) = rng.pick(&keys).and_then(|k| record(k)) {
                let entry = pick_str(
                    rng,
                    &[
                        "v", "active", "kind", "align", "p:asset", "p:width", "p:height", "p:alt",
                        "p:field", "p:target", "p:text", "p:zz", "zz",
                    ],
                );
                let value = match entry.as_str() {
                    "v" => scalar_or_text(rng, "1".to_owned()),
                    "active" => scalar_or_text(rng, "true".to_owned()),
                    _ => inline_entry_value(rng, &entry),
                };
                rec.insert(&entry, value)?;
            }
        }
        3 => {
            if let Some(key) = rng.pick(&keys) {
                if rng.chance(50) {
                    map.delete(key)?;
                } else if let Some(rec) = record(key) {
                    rec.delete(&pick_str(
                        rng,
                        &["v", "active", "kind", "p:asset", "p:field"],
                    ))?;
                }
            }
        }
        4 => {
            // A record keyed to a real host, with a character that may not exist.
            let hosts = ids(&doc.doc.get_tree("content"));
            if let Some(host) = rng.pick(&hosts) {
                let peer = host.split('@').nth(1).unwrap_or("1");
                let key = format!("{host}/{}@{peer}", rng.below(400));
                let rec = map.insert_container(&key, LoroMap::new())?;
                rec.insert("v", 1)?;
                rec.insert("active", true)?;
                rec.insert("kind", pick_str(rng, &["image", "field", "future-kind"]))?;
                rec.insert("p:field", "page-number")?;
            }
        }
        5 => {
            let key = pick_str(rng, &["", "x", "0@0/0@0", "/", "a/b/c", "1@1/", "/1@1"]);
            if rng.chance(50) {
                let rec = map.insert_container(&key, LoroMap::new())?;
                rec.insert("v", 1)?;
                rec.insert("active", true)?;
                rec.insert("kind", "field")?;
            } else {
                map.insert(&key, value(rng, doc))?;
            }
        }
        _ => {
            // The same name, another container type (a list would be outside
            // the sync packet vocabulary, and refused before it got here).
            doc.doc
                .get_text("inline1")
                .insert(0, "text where a map belongs")?;
        }
    }
    Ok(())
}

fn envelope(rng: &mut Rng, doc: &Document) -> String {
    let contents = ids(&doc.doc.get_tree("content"));
    let ranges = ids(&doc.doc.get_tree("ranges"));
    match rng.below(10) {
        0 => {
            let node = contents
                .get(rng.below(contents.len()))
                .cloned()
                .unwrap_or_default();
            format!(r#"{{"version":1,"kind":"link","payload":{{"type":"node","node":"{node}"}}}}"#)
        }
        1 => {
            let range = ranges
                .get(rng.below(ranges.len()))
                .cloned()
                .unwrap_or_default();
            format!(
                r#"{{"version":1,"kind":"link","payload":{{"type":"range","range":"{range}"}}}}"#
            )
        }
        2 => format!(
            r#"{{"version":1,"kind":"comment","payload":{{"thread":"{}"}}}}"#,
            "t".repeat(rng.below(20_000))
        ),
        3 => format!(
            r#"{{"version":1,"kind":"link","payload":{{"type":"url","url":"https://e.com/{}"}}}}"#,
            "a".repeat(rng.below(20_000))
        ),
        4 => "[".repeat(1 + rng.below(5_000)),
        _ => pick_str(rng, ENVELOPES),
    }
}

fn annotation_op(doc: &Document, rng: &mut Rng) -> loro::LoroResult<()> {
    let ranges = doc.doc.get_tree("ranges");
    let all = nodes(&ranges);
    let Some(&target) = rng.pick(&all) else {
        return Ok(());
    };
    let meta = ranges.get_meta(target)?;
    match rng.below(8) {
        0..=3 => meta.insert("annot1", envelope(rng, doc))?,
        4 => meta.insert("annot1", scalar_or_text(rng, "{}".to_owned()))?,
        5 => meta.insert("annot-restore1", pick_str(rng, HINTS))?,
        6 => meta.delete("annot1")?,
        _ => {
            // A fresh range with only an envelope: no anchors at all.
            let id = ranges.create(TreeParentId::Root)?;
            ranges.get_meta(id)?.insert("annot1", envelope(rng, doc))?;
        }
    }
    Ok(())
}

fn list_value(rng: &mut Rng, key: &str, lists: &[String], contents: &[String]) -> LoroValue {
    let text = match key {
        "base" => {
            let mut pool: Vec<&str> = lists.iter().map(String::as_str).collect();
            pool.extend(contents.iter().map(String::as_str));
            pool.extend(["0@0", ""]);
            pick_str(rng, &pool)
        }
        k if k.ends_with(".format") => pick_str(rng, FORMATS),
        k if k.ends_with(".text") => pick_str(rng, TEMPLATES),
        k if k.ends_with(".start") => pick_str(rng, NUMBERS),
        k if k.ends_with(".indent") || k.ends_with(".hanging") => pick_str(rng, NUMBERS),
        k if k.ends_with(".marker") => pick_str(
            rng,
            &[
                r#"{"weight":700}"#,
                r#"{"size":-5}"#,
                r#"{"version":1,"weight":9999}"#,
                "{",
                "null",
                "",
            ],
        ),
        _ => "1".to_owned(),
    };
    scalar_or_text(rng, text)
}

fn list_op(doc: &Document, rng: &mut Rng) -> loro::LoroResult<()> {
    let lists = doc.doc.get_tree("lists");
    let all = nodes(&lists);
    let list_ids = ids(&lists);
    let contents = ids(&doc.doc.get_tree("content"));
    match rng.below(7) {
        0 => {
            let id = lists.create(TreeParentId::Root)?;
            let meta = lists.get_meta(id)?;
            meta.insert("v", 1)?;
            meta.insert("0.format", pick_str(rng, FORMATS))?;
            meta.insert("0.text", pick_str(rng, TEMPLATES))?;
        }
        1..=4 => {
            if let Some(&node) = rng.pick(&all) {
                let key = match rng.below(6) {
                    0 => pick_str(rng, &["v", "deleted"]),
                    1 => "base".to_owned(),
                    _ => format!(
                        "{}.{}",
                        rng.below(12),
                        pick_str(
                            rng,
                            &["format", "text", "start", "indent", "hanging", "marker"]
                        )
                    ),
                };
                let value = list_value(rng, &key, &list_ids, &contents);
                lists.get_meta(node)?.insert(&key, value)?;
            }
        }
        5 => {
            if let Some(&node) = rng.pick(&all) {
                lists.delete(node)?;
            }
        }
        _ => {
            if let (Some(&a), Some(&b)) = (rng.pick(&all), rng.pick(&all)) {
                lists.mov(a, TreeParentId::Node(b))?;
            }
        }
    }
    Ok(())
}

fn paragraph_value(rng: &mut Rng, key: &str, lists: &[String], contents: &[String]) -> LoroValue {
    let text = match key {
        "role1" => pick_str(
            rng,
            &[
                "body",
                "heading-1",
                "heading-6",
                "heading-0",
                "heading-7",
                "heading-99",
                "title",
                "subtitle",
                "quote",
                "code",
                "verse",
                "unknown",
                "",
                "HEADING-1",
            ],
        ),
        "list1" => {
            let mut pool: Vec<&str> = lists.iter().map(String::as_str).collect();
            pool.extend(contents.iter().map(String::as_str));
            pool.extend(["0@0", ""]);
            pick_str(rng, &pool)
        }
        "level1" => pick_str(rng, &["0", "1", "8", "9", "255", "-1", "x", ""]),
        "restart1" => pick_str(rng, NUMBERS),
        "style" => {
            let name = pick_str(rng, STYLE_NAMES);
            match rng.below(4) {
                0 => format!("\u{1}style1:{}", rng.next_u64() % 0xfff),
                1 => format!("\u{2}builtin1:{name}"),
                _ => name,
            }
        }
        _ => pick_str(rng, STYLE_NAMES),
    };
    // Level and restart are numbers in a well-formed record.
    match (key, rng.below(3)) {
        ("level1", 0) => LoroValue::I64(*rng.pick(&[0, 3, 8, 9, 255, -1, i64::MAX]).unwrap_or(&0)),
        ("restart1", 0) => LoroValue::I64(*rng.pick(&[0, 1, -5, i64::MAX, i64::MIN]).unwrap_or(&0)),
        _ => scalar_or_text(rng, text),
    }
}

fn paragraph_op(doc: &Document, rng: &mut Rng) -> loro::LoroResult<()> {
    let content = doc.doc.get_tree("content");
    let contents = ids(&content);
    let list_ids = ids(&doc.doc.get_tree("lists"));
    let key = pick_str(
        rng,
        &[
            "role1", "list1", "level1", "restart1", "role1", "list1", "style",
        ],
    );
    let value = paragraph_value(rng, &key, &list_ids, &contents);
    if rng.chance(70) {
        if let Some(&node) = rng.pick(&nodes(&content)) {
            let meta = content.get_meta(node)?;
            if rng.chance(15) {
                meta.delete(&key)?;
            } else {
                meta.insert(&key, value)?;
            }
        }
    } else {
        // A flow break's record carries the same keys.
        let records = doc.break_records();
        let keys: Vec<String> = records.keys().map(|k| k.to_string()).collect();
        if let Some(ValueOrContainer::Container(Container::Map(rec))) =
            rng.pick(&keys).and_then(|k| records.get(k))
        {
            rec.insert(&key, value)?;
        }
    }
    Ok(())
}

fn style_op(doc: &Document, rng: &mut Rng) -> loro::LoroResult<()> {
    let name = pick_str(rng, STYLE_NAMES);
    match rng.below(8) {
        0..=2 => {
            let property = pick_str(rng, PROPERTIES);
            let text = pick_str(rng, EXPRESSIONS);
            let value = scalar_or_text(rng, text);
            doc.doc
                .get_map("styles1")
                .insert(&format!("{property}/{name}"), value)?;
        }
        3 | 4 => {
            let text = match rng.below(5) {
                0 => "defined".to_owned(),
                1 => "deleted".to_owned(),
                2 => format!("renamed:{}", pick_str(rng, STYLE_NAMES)),
                3 => format!("renamed:{name}"),
                _ => "garbage".to_owned(),
            };
            let value = scalar_or_text(rng, text);
            doc.doc
                .get_map("styles1")
                .insert(&format!("state/{name}"), value)?;
        }
        5 => {
            // A per-block patch, keyed by the node.
            let contents = ids(&doc.doc.get_tree("content"));
            let node = contents
                .get(rng.below(contents.len()))
                .cloned()
                .unwrap_or_default();
            let property = pick_str(rng, PROPERTIES);
            let text = pick_str(rng, EXPRESSIONS);
            let value = scalar_or_text(rng, text);
            doc.doc
                .get_map("para1")
                .insert(&format!("{node}/{property}"), value)?;
        }
        _ => {
            let map = doc.doc.get_map("styles2");
            let owner = format!("\u{1}style1:{:x}", rng.next_u64() % 0xffff);
            let (key, value) = match rng.below(6) {
                0 => (
                    format!("clock/{}", rng.below(300)),
                    scalar_or_text(rng, "5".to_owned()),
                ),
                1 => (format!("label/{owner}"), scalar_or_text(rng, name)),
                2 => (format!("alias/{name}"), scalar_or_text(rng, owner)),
                3 => (
                    format!("base/{owner}"),
                    scalar_or_text(rng, "builtin1".to_owned()),
                ),
                4 => (
                    "legacy-builtins1".to_owned(),
                    scalar_or_text(rng, "1".to_owned()),
                ),
                _ => (
                    pick_str(rng, &["", "x", "clock/", "label/", "alias/"]),
                    LoroValue::Null,
                ),
            };
            map.insert(&key, value)?;
        }
    }
    Ok(())
}

/// Identity roots. Corrupting `document1` itself is left to named tests: the
/// attacker's own export reads its stored lineage and would refuse, so the
/// packet could never be sent. What a peer can send is the index and a
/// declaration of the lineage it is joining.
fn identity_op(doc: &Document, rng: &mut Rng) -> loro::LoroResult<()> {
    if rng.chance(50) {
        let key = pick_str(rng, &["x", "", "00112233445566778899aabbccddeeff"]);
        let value = scalar_or_text(rng, "1".to_owned());
        doc.doc.get_map("lineage-index1").insert(&key, value)?;
    } else {
        let id = "00112233445566778899aabbccddeeff";
        doc.doc
            .get_map(format!("document1:{id}").as_str())
            .insert("id", id)?;
    }
    Ok(())
}

fn marker_op(doc: &Document, rng: &mut Rng) -> loro::LoroResult<()> {
    let all = texts(doc);
    let Some(text) = rng.pick(&all) else {
        return Ok(());
    };
    let len = text.len_unicode();
    match rng.below(3) {
        // A bare object marker, with no record.
        0 => text.insert(rng.below(len + 1), "\u{fffc}")?,
        1 if len > 0 => {
            let at = rng.below(len);
            text.delete(at, 1)?;
        }
        _ => text.insert(rng.below(len + 1), "\u{fffc}\u{fffc}\u{fffc}")?,
    }
    Ok(())
}

/// Writes one string under `key` of the root map `root`, as a peer that
/// bypasses the kernel could. For tests that pin a specific attack. Commits.
pub fn write_root_string(doc: &Document, root: &str, key: &str, raw: &str) -> loro::LoroResult<()> {
    doc.doc.get_map(root).insert(key, raw)?;
    doc.commit();
    Ok(())
}

/// Writes one entry of the metadata of the node `id` (`counter@peer`) of the
/// tree `tree` (`content`, `ranges`, `lists` ...): a string, or an integer
/// when `raw` is `i64:<number>`. Commits.
pub fn write_node_entry(
    doc: &Document,
    tree: &str,
    id: &str,
    key: &str,
    raw: &str,
) -> loro::LoroResult<()> {
    let tree = doc.doc.get_tree(tree);
    let id = TreeID::try_from(id).map_err(|_| loro::LoroError::ArgErr("tree id".into()))?;
    let meta = tree.get_meta(id)?;
    match raw.strip_prefix("i64:").map(str::parse::<i64>) {
        Some(Ok(n)) => meta.insert(key, n)?,
        _ => meta.insert(key, raw)?,
    }
    doc.commit();
    Ok(())
}

/// Writes one entry of the inline record at `key` (`host/char`), creating a
/// readable active record first when there is none. `raw` is a string, or an
/// integer for `i64:<n>`. Commits.
pub fn write_inline_entry(
    doc: &Document,
    key: &str,
    entry: &str,
    raw: &str,
) -> loro::LoroResult<()> {
    let map = doc.inline_map();
    let rec = match map.get(key) {
        Some(ValueOrContainer::Container(Container::Map(m))) => m,
        _ => {
            let rec = map.insert_container(key, LoroMap::new())?;
            rec.insert("v", 1)?;
            rec.insert("active", true)?;
            rec.insert("kind", "field")?;
            rec
        }
    };
    match raw.strip_prefix("i64:").map(str::parse::<i64>) {
        Some(Ok(n)) => rec.insert(entry, n)?,
        _ => rec.insert(entry, raw)?,
    }
    doc.commit();
    Ok(())
}

/// The inline record map as text, one `key => value` per record, for a test
/// that has to explain what an attack left in the store.
pub fn dump_inline_records(doc: &Document) -> Vec<String> {
    let map = doc.inline_map();
    let mut out: Vec<String> = map
        .keys()
        .map(|k| {
            let key = k.to_string();
            let value = map.get(&key).map(|v| v.get_deep_value());
            format!("{key} => {value:?}")
        })
        .collect();
    out.sort();
    out
}
