//! A hostile peer for tests (feature `hostile-peer`): seeded raw store
//! operations that bypass the editing kernel entirely.
//!
//! It writes whatever the CRDT can represent into the engine's containers:
//! wrong value types, unknown keys and kinds, texts in the wrong place,
//! extreme numbers, tree moves and physical deletes, and records that are
//! garbage or almost right. Its edits reach a victim only through
//! [`Document::export_delta`], exactly as a remote peer's would.

use loro::{
    Container, LoroList, LoroMap, LoroText, LoroTree, LoroValue, TreeID, TreeParentId,
    ValueOrContainer,
};

use crate::Document;

/// A small deterministic generator (SplitMix64), so runs reproduce by seed.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed)
    }

    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n`; `0` when `n` is zero.
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            return 0;
        }
        usize::try_from(self.next() % u64::try_from(n).unwrap_or(u64::MAX)).unwrap_or(0)
    }

    pub fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }

    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> Option<&'a T> {
        items.get(self.below(items.len()))
    }
}

const KEYS: &[&str] = &[
    "kind",
    "style",
    "text",
    "deleted",
    "overrides",
    "image1",
    "table1",
    "json",
    "node",
    "start",
    "end",
    "policy1",
    "empty",
    "parent",
    "size",
    "family",
    "succ/0@1",
    "pred/0@1",
    "zz-unknown",
];

const STRINGS: &[&str] = &[
    "paragraph",
    "annotation",
    "image",
    "table",
    "unknown-kind",
    "",
    "body",
    "note",
    "keep",
    "missing",
    "0@1",
    "2147483647@18446744073709551614",
    "{",
    r#"{"version":1,"role":{"table":{"columns":[]}}}"#,
    r#"{"version":1,"role":{"row":{"header":true}}}"#,
    r#"{"version":1,"role":{"cell":{"column":4294967295}}}"#,
    r#"{"version":1,"image":{"asset":"00","width":null,"height":null}}"#,
    r#"{"version":1,"start":"after","end":"before","empty":"keep"}"#,
    r#"{"schema":"reprise.follow","owner":"0@1","targets":{},"params":{}}"#,
    r#"{"schema":"reprise.reading-order","targets":{"order":[{"node":"0@1"}]}}"#,
    "\u{202e}\u{0301}\u{fe0f}\u{1f469}\u{200d}\u{1f467}",
    "\0",
];

/// Writes `ops` random raw operations into `doc`'s store and commits them.
/// They stay inside the `delta-json` vocabulary (map, text and tree
/// operations); [`scribble_outside`] writes the rest.
/// `doc` should be a fork of the victim with its own peer.
pub fn scribble(doc: &Document, rng: &mut Rng, ops: usize) {
    for _ in 0..ops {
        let _ = one(doc, rng);
        if rng.chance(20) {
            doc.commit();
        }
    }
    doc.commit();
}

fn trees(doc: &Document) -> [LoroTree; 3] {
    [
        doc.doc.get_tree("content"),
        doc.doc.get_tree("ranges"),
        doc.doc.get_tree("relations"),
    ]
}

fn nodes(tree: &LoroTree) -> Vec<TreeID> {
    let mut all = tree.get_nodes(true);
    all.sort_by_key(|n| n.id);
    all.into_iter().map(|n| n.id).collect()
}

fn texts(doc: &Document) -> Vec<LoroText> {
    let mut out = Vec::new();
    let tree = doc.doc.get_tree("content");
    for id in nodes(&tree) {
        if let Ok(meta) = tree.get_meta(id)
            && let Some(ValueOrContainer::Container(Container::Text(t))) = meta.get("text")
        {
            out.push(t);
        }
    }
    out
}

fn value(rng: &mut Rng, doc: &Document) -> LoroValue {
    match rng.below(12) {
        0 => LoroValue::Null,
        1 => LoroValue::Bool(rng.chance(50)),
        2 => LoroValue::I64(*rng.pick(&[i64::MIN, -1, 0, 1, i64::MAX]).unwrap_or(&0)),
        3 => LoroValue::Double(
            *rng.pick(&[f64::NAN, f64::INFINITY, -0.0, 1e308])
                .unwrap_or(&0.0),
        ),
        4 => LoroValue::Binary(
            (0..rng.below(40))
                .map(|_| (rng.next() & 0xff) as u8)
                .collect::<Vec<_>>()
                .into(),
        ),
        5 => {
            // An encoded anchor from a real text: right shape, maybe wrong place.
            let texts = texts(doc);
            match rng.pick(&texts) {
                Some(t) => {
                    let len = t.len_unicode();
                    let side = if rng.chance(50) {
                        loro::cursor::Side::Left
                    } else {
                        loro::cursor::Side::Right
                    };
                    t.get_cursor(rng.below(len.saturating_add(1)), side)
                        .map(|c| LoroValue::Binary(c.encode().into()))
                        .unwrap_or(LoroValue::Null)
                }
                None => LoroValue::Null,
            }
        }
        6 => {
            // An ID of some node, as a string.
            let all: Vec<TreeID> = trees(doc).iter().flat_map(nodes).collect();
            rng.pick(&all).map_or(LoroValue::Null, |id| {
                LoroValue::String(id.to_string().into())
            })
        }
        7 => LoroValue::String("x".repeat(rng.below(5000)).into()),
        8 => LoroValue::List(vec![LoroValue::I64(1), LoroValue::Null].into()),
        _ => LoroValue::String((*rng.pick(STRINGS).unwrap_or(&"")).into()),
    }
}

fn write_map(map: &LoroMap, rng: &mut Rng, doc: &Document) -> loro::LoroResult<()> {
    let key = *rng.pick(KEYS).unwrap_or(&"kind");
    match rng.below(10) {
        0 => {
            map.delete(key)?;
        }
        1 => {
            let t = map.insert_container(key, LoroText::new())?;
            t.insert(0, rng.pick(STRINGS).unwrap_or(&""))?;
        }
        2 => {
            let m = map.insert_container(key, LoroMap::new())?;
            m.insert(rng.pick(KEYS).unwrap_or(&""), value(rng, doc))?;
        }
        3 => {
            map.insert_container(key, LoroList::new())?;
        }
        _ => map.insert(key, value(rng, doc))?,
    }
    Ok(())
}

fn one(doc: &Document, rng: &mut Rng) -> loro::LoroResult<()> {
    let [content, ranges, relations] = trees(doc);
    let tree = match rng.below(6) {
        0 => ranges,
        1 => relations,
        _ => content,
    };
    let all = nodes(&tree);
    let target = rng.pick(&all).copied();
    match rng.below(14) {
        0 => {
            let parent = match rng.pick(&all) {
                Some(p) if rng.chance(50) => TreeParentId::Node(*p),
                _ => TreeParentId::Root,
            };
            let id = tree.create(parent)?;
            let meta = tree.get_meta(id)?;
            meta.insert("kind", *rng.pick(STRINGS).unwrap_or(&"paragraph"))?;
            if rng.chance(70) {
                meta.insert_container("text", LoroText::new())?
                    .insert(0, rng.pick(STRINGS).unwrap_or(&""))?;
            }
        }
        1 => {
            if let (Some(t), Some(p)) = (target, rng.pick(&all)) {
                tree.mov(t, TreeParentId::Node(*p))?;
            }
        }
        2 => {
            if let Some(t) = target {
                if rng.chance(50) {
                    tree.mov(t, TreeParentId::Root)?;
                } else {
                    tree.mov_to(t, TreeParentId::Root, rng.below(4))?;
                }
            }
        }
        3 => {
            if let Some(t) = target {
                tree.delete(t)?;
            }
        }
        4..=7 => {
            if let Some(t) = target {
                write_map(&tree.get_meta(t)?, rng, doc)?;
            }
        }
        8..=10 => {
            let texts = texts(doc);
            if let Some(t) = rng.pick(&texts) {
                let len = t.len_unicode();
                match rng.below(3) {
                    0 => t.insert(rng.below(len + 1), rng.pick(STRINGS).unwrap_or(&""))?,
                    1 if len > 0 => {
                        let at = rng.below(len);
                        t.delete(at, rng.below(len - at) + 1)?;
                    }
                    _ => {}
                }
            }
        }
        11 => {
            let styles = doc.doc.get_map("styles");
            let name = *rng.pick(&["", "body", "note", "loop"]).unwrap_or(&"body");
            if rng.chance(30) {
                styles.insert(name, value(rng, doc))?;
            } else {
                let style = styles.insert_container(name, LoroMap::new())?;
                write_map(&style, rng, doc)?;
                style.insert(
                    "parent",
                    *rng.pick(&["", "body", "note", "loop"]).unwrap_or(&""),
                )?;
            }
        }
        12 => {
            let root = *rng
                .pick(&["page_setup", "page_templates", "zz-unknown-root"])
                .unwrap_or(&"zz-unknown-root");
            write_map(&doc.doc.get_map(root), rng, doc)?;
        }
        _ => {
            let unknown = doc.doc.get_map("zz-unknown-root");
            unknown.insert_container("list", LoroList::new())?;
        }
    }
    Ok(())
}

/// Writes one operation outside the `delta-json` vocabulary: a text mark,
/// a list insertion or a movable list insertion. Sync must refuse it.
pub fn scribble_outside(doc: &Document, rng: &mut Rng) {
    let marked = rng.below(3) == 0
        && texts(doc)
            .into_iter()
            .filter(|t| t.len_unicode() > 0)
            .any(|t| t.mark(0..t.len_unicode(), "bold", true).is_ok());
    if !marked {
        let written = if rng.chance(50) {
            doc.doc.get_list("zz-list").insert(0, 1)
        } else {
            doc.doc.get_movable_list("zz-movable").insert(0, 1)
        };
        debug_assert!(written.is_ok());
    }
    doc.commit();
}
