use std::cell::RefCell;

use reprise_compose::{
    Available, ComposeRequest, Composer, Composition, GeometryProvider, Interval, LineQuery,
};
use reprise_diag::Note;
use reprise_geom::Length;

use crate::{CallContext, Phase, Plugin, abi, codes};

/// Applies a plugin shape within existing frame room. Never expands that room.
pub struct PluginGeometry<'a> {
    pub plugin: Option<&'a Plugin>,
    pub operation: u32,
    pub fallback: &'a dyn GeometryProvider,
    notes: RefCell<Vec<Note>>,
}

impl<'a> PluginGeometry<'a> {
    pub fn new(
        plugin: Option<&'a Plugin>,
        operation: u32,
        fallback: &'a dyn GeometryProvider,
    ) -> Self {
        Self {
            plugin,
            operation,
            fallback,
            notes: RefCell::new(Vec::new()),
        }
    }

    pub fn notes(&self) -> Vec<Note> {
        self.notes.borrow().clone()
    }

    fn room(&self, query: &LineQuery<'_>, base: &[Interval]) -> Result<Available, Note> {
        let Some(plugin) = self.plugin else {
            return Err(Note::warning(
                codes::UNAVAILABLE,
                "plugin geometry unavailable; frame measure used",
            ));
        };
        if plugin.envelope().phase != Phase::Layout {
            return Err(Note::warning(
                codes::CAPABILITY,
                "editing plugins cannot provide layout geometry",
            ));
        }
        if base.len() > abi::MAX_INTERVALS || query.previous.len() > 4096 {
            return Err(Note::warning(
                codes::LIMIT,
                "geometry request exceeded its record limit",
            ));
        }
        let mut input = Vec::new();
        for v in [
            query.line as i32,
            query.block_offset.0,
            query.line_height.0,
            base.len() as i32,
        ] {
            abi::push_i32(&mut input, v);
        }
        for i in base {
            abi::push_i32(&mut input, i.start.0);
            abi::push_i32(&mut input, i.end.0);
        }
        abi::push_i32(&mut input, query.previous.len() as i32);
        for previous in query.previous {
            let start = u32::try_from(previous.text.start).map_err(|_| abi::bad())?;
            let end = u32::try_from(previous.text.end).map_err(|_| abi::bad())?;
            for v in [
                previous.line as i32,
                previous.block_offset.0,
                previous.height.0,
                previous.available.start.0,
                previous.available.end.0,
                start as i32,
                end as i32,
            ] {
                abi::push_i32(&mut input, v);
            }
        }
        let bytes = plugin
            .call(self.operation, &input, &CallContext::default())?
            .bytes;
        if bytes.len() < 8 {
            return Err(abi::bad());
        }
        let tag = abi::read_i32(&bytes, 0)?;
        let n = abi::read_i32(&bytes, 4)?;
        match tag {
            0 if n >= 0 && n as usize <= abi::MAX_INTERVALS => {
                let n = n as usize;
                if bytes.len() != 8 + n * 8 {
                    return Err(abi::bad());
                }
                let mut result = Vec::with_capacity(n);
                let mut last = None;
                for j in 0..n {
                    let start = Length(abi::read_i32(&bytes, 8 + j * 8)?);
                    let end = Length(abi::read_i32(&bytes, 12 + j * 8)?);
                    if start >= end
                        || last.is_some_and(|p| p > start)
                        || !base.iter().any(|i| i.start <= start && end <= i.end)
                    {
                        return Err(abi::bad());
                    }
                    last = Some(end);
                    result.push(Interval::new(start, end));
                }
                Ok(Available::Room(result))
            }
            1 if bytes.len() == 8 && n > query.block_offset.0 => {
                Ok(Available::Skip { next: Length(n) })
            }
            2 if bytes.len() == 8 && n == 0 => Ok(Available::End),
            _ => Err(abi::bad()),
        }
    }
}

impl GeometryProvider for PluginGeometry<'_> {
    fn available(&self, query: &LineQuery<'_>) -> Available {
        let fallback = self.fallback.available(query);
        let Available::Room(base) = &fallback else {
            return fallback;
        };
        match self.room(query, base) {
            Ok(room) => room,
            Err(note) => {
                // One note per distinct failure per composition, deterministic query order.
                let mut notes = self.notes.borrow_mut();
                if !notes.contains(&note) {
                    notes.push(note);
                }
                fallback
            }
        }
    }
}

/// Adapter around an existing conforming composer, with the frame's own measure as fallback.
pub struct GeometryComposer {
    pub plugin: Option<Plugin>,
    pub operation: u32,
    pub composer: Box<dyn Composer>,
}

impl Composer for GeometryComposer {
    fn name(&self) -> &'static str {
        "plugin-geometry"
    }
    fn compose(&self, request: &ComposeRequest<'_>) -> Composition {
        let geometry = PluginGeometry::new(self.plugin.as_ref(), self.operation, request.geometry);
        let mut result = self.composer.compose(&ComposeRequest {
            geometry: &geometry,
            ..*request
        });
        result.notes.extend(geometry.notes());
        result
    }
}
