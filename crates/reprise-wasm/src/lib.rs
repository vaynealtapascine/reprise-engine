//! Typed JavaScript boundary. All engine work is delegated to the native facade.
#![forbid(unsafe_code)]
use reprise::{Error, Payload};
use serde::{Serialize, de::DeserializeOwned};
use std::collections::BTreeMap;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(typescript_custom_section)]
const TYPES: &str = include_str!("../ts/types.d.ts");

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(typescript_type = "Payload<SyncRequest>")]
    pub type SyncRequestPayload;
    #[wasm_bindgen(typescript_type = "Payload<SyncPacket>")]
    pub type SyncPacketPayload;
    #[wasm_bindgen(typescript_type = "Payload<SyncReport>")]
    pub type SyncReportPayload;
    #[wasm_bindgen(typescript_type = "Payload<StableSelection>")]
    pub type StableSelectionPayload;
    #[wasm_bindgen(typescript_type = "Payload<Selection | null>")]
    pub type OptionalSelectionPayload;
    #[wasm_bindgen(typescript_type = "Payload<StableSelection | null>")]
    pub type OptionalStableSelectionPayload;
    #[wasm_bindgen(typescript_type = "Payload<EditReport>")]
    pub type EditReportPayload;
    #[wasm_bindgen(typescript_type = "Payload<Presence>")]
    pub type PresencePayload;
    #[wasm_bindgen(typescript_type = "Payload<PresenceView>")]
    pub type PresenceViewPayload;

    #[wasm_bindgen(typescript_type = "Payload<CopyAs>")]
    pub type CopyAsPayload;
    #[wasm_bindgen(typescript_type = "Payload<Array<Resource>>")]
    pub type ListResourcePayload;
    #[wasm_bindgen(typescript_type = "Payload<Applied>")]
    pub type AppliedPayload;
    #[wasm_bindgen(typescript_type = "Payload<AssetDeclaration>")]
    pub type AssetDeclarationPayload;
    #[wasm_bindgen(typescript_type = "Payload<Awareness>")]
    pub type AwarenessPayload;
    #[wasm_bindgen(typescript_type = "Payload<Bytes>")]
    pub type BytesPayload;
    #[wasm_bindgen(typescript_type = "Payload<Caret>")]
    pub type CaretPayload;
    #[wasm_bindgen(typescript_type = "Payload<Create>")]
    pub type CreatePayload;
    #[wasm_bindgen(typescript_type = "Payload<Cursor>")]
    pub type CursorPayload;
    #[wasm_bindgen(typescript_type = "Payload<DisplayPage>")]
    pub type DisplayPagePayload;
    #[wasm_bindgen(typescript_type = "Payload<ExportFormat>")]
    pub type ExportFormatPayload;
    #[wasm_bindgen(typescript_type = "Payload<Exported>")]
    pub type ExportedPayload;
    #[wasm_bindgen(typescript_type = "Payload<Face>")]
    pub type FacePayload;
    #[wasm_bindgen(typescript_type = "Payload<FontDeclaration>")]
    pub type FontDeclarationPayload;
    #[wasm_bindgen(typescript_type = "Payload<Hit>")]
    pub type HitPayload;
    #[wasm_bindgen(typescript_type = "Payload<HitTest>")]
    pub type HitTestPayload;
    #[wasm_bindgen(typescript_type = "Payload<LayoutOptions>")]
    pub type LayoutOptionsPayload;
    #[wasm_bindgen(typescript_type = "Payload<LayoutProgress>")]
    pub type LayoutProgressPayload;
    #[wasm_bindgen(typescript_type = "Payload<Move>")]
    pub type MovePayload;
    #[wasm_bindgen(typescript_type = "Payload<Open>")]
    pub type OpenPayload;
    #[wasm_bindgen(typescript_type = "Payload<PageRect>")]
    pub type PageRectPayload;
    #[wasm_bindgen(typescript_type = "Payload<Paste>")]
    pub type PastePayload;
    #[wasm_bindgen(typescript_type = "Payload<PluginEdit>")]
    pub type PluginEditPayload;
    #[wasm_bindgen(typescript_type = "Payload<PluginInstall>")]
    pub type PluginInstallPayload;
    #[wasm_bindgen(typescript_type = "Payload<PluginSpec>")]
    pub type PluginSpecPayload;
    #[wasm_bindgen(typescript_type = "Payload<Selection>")]
    pub type SelectionPayload;
    #[wasm_bindgen(typescript_type = "Payload<State>")]
    pub type StatePayload;
    #[wasm_bindgen(typescript_type = "Payload<string>")]
    pub type StringPayload;
    #[wasm_bindgen(typescript_type = "Payload<SyncInfo>")]
    pub type SyncInfoPayload;
    #[wasm_bindgen(typescript_type = "Payload<SyncUpdate>")]
    pub type SyncUpdatePayload;
    #[wasm_bindgen(typescript_type = "Payload<TextImport>")]
    pub type TextImportPayload;
    #[wasm_bindgen(typescript_type = "Payload<Transaction>")]
    pub type TransactionPayload;
    #[wasm_bindgen(typescript_type = "Payload<ImageInsert>")]
    pub type ImageInsertPayload;
    #[wasm_bindgen(typescript_type = "Payload<Array<Diagnostic>>")]
    pub type ListDiagnosticPayload;
    #[wasm_bindgen(typescript_type = "Payload<Array<PageRect>>")]
    pub type ListPageRectPayload;
    #[wasm_bindgen(typescript_type = "Payload<Array<ReadingStep>>")]
    pub type ListReadingStepPayload;
    #[wasm_bindgen(typescript_type = "Payload<boolean>")]
    pub type BooleanPayload;
    #[wasm_bindgen(typescript_type = "Payload<number>")]
    pub type HandlePayload;
}
thread_local! {
    static PANIC_HANDLER: std::cell::RefCell<Option<js_sys::Function>> =
        const { std::cell::RefCell::new(None) };
}

/// Calls `handler(message)` when the engine panics, before the instance
/// traps. A `panic=abort` build cannot recover: after the call every export
/// fails, so the host should discard this instance (its worker), reopen the
/// last saved package in a fresh one and resynchronise. The message names
/// the panic and its location, for reporting.
#[wasm_bindgen(js_name = setPanicHandler)]
pub fn set_panic_handler(handler: js_sys::Function) {
    PANIC_HANDLER.with(|h| *h.borrow_mut() = Some(handler));
    static INSTALL: std::sync::Once = std::sync::Once::new();
    INSTALL.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let message = JsValue::from_str(&info.to_string());
            PANIC_HANDLER.with(|h| {
                if let Ok(h) = h.try_borrow()
                    && let Some(f) = h.as_ref()
                {
                    let _ = f.call1(&JsValue::NULL, &message);
                }
            });
            previous(info);
        }));
    });
}

fn error(e: Error) -> JsValue {
    e.payload()
        .serialize(
            &serde_wasm_bindgen::Serializer::new()
                .serialize_missing_as_null(true)
                .serialize_maps_as_objects(true),
        )
        .unwrap_or_else(|_| JsValue::from_str(e.code()))
}
fn encode<T: Serialize>(r: reprise::Result<T>) -> Result<JsValue, JsValue> {
    r.map_err(error)?
        .serialize(
            &serde_wasm_bindgen::Serializer::new()
                .serialize_missing_as_null(true)
                .serialize_maps_as_objects(true),
        )
        .map_err(|e| error(Error::Invalid(e.to_string())))
}
fn bytes(v: &js_sys::Uint8Array, max: u32) -> Result<Vec<u8>, JsValue> {
    let value: &JsValue = v.as_ref();
    if !value.is_instance_of::<js_sys::Uint8Array>() {
        return Err(error(Error::Invalid("bytes must be Uint8Array".into())));
    }
    if v.length() > max {
        return Err(error(Error::Limit("byte input".into())));
    }
    Ok(v.to_vec())
}
fn decode<T: DeserializeOwned>(value: &JsValue) -> Result<Payload<T>, JsValue> {
    let version = js_sys::Reflect::get(value, &JsValue::from_str("version"))
        .map_err(|_| error(Error::Invalid("missing version".into())))?
        .as_f64()
        .ok_or_else(|| error(Error::Invalid("version must be integer".into())))?;
    if version != f64::from(reprise::API_VERSION) {
        return Err(error(
            if version >= 0.0 && version <= f64::from(u32::MAX) && version.fract() == 0.0 {
                Error::Version(version as u32)
            } else {
                Error::Invalid("version must be u32".into())
            },
        ));
    }
    // Copy the bounded object tree once. Getters cannot change the payload after
    // preflight, and serde only sees plain objects, arrays and owned byte buffers.
    let seen = js_sys::Set::new(&JsValue::UNDEFINED);
    let holder: JsValue = js_sys::Object::new().into();
    let mut pending = vec![(
        value.clone(),
        holder.clone(),
        JsValue::from_str("payload"),
        0u32,
        false,
    )];
    let mut count = 0u32;
    let mut units = 0u32;
    let mut byte_count = 0u32;
    while let Some((v, parent, key, depth, leaving)) = pending.pop() {
        if leaving {
            seen.delete(&v);
            continue;
        }
        count = count.saturating_add(1);
        if count > 100_000 || depth > 64 {
            return Err(error(Error::Limit("JS nodes/depth".into())));
        }
        let set = |v: &JsValue| {
            js_sys::Reflect::set(&parent, &key, v)
                .map_err(|_| error(Error::Invalid("object snapshot".into())))
        };
        if v.is_string() {
            units = units.saturating_add(js_sys::JsString::from(v.clone()).length());
            if units > reprise::MAX_PAYLOAD_BYTES as u32 {
                return Err(error(Error::Limit("JS string units".into())));
            }
            set(&v)?;
            continue;
        }
        if !v.is_object() || v.is_null() {
            set(&v)?;
            continue;
        }
        if let Some(array) = v.dyn_ref::<js_sys::Uint8Array>() {
            byte_count = byte_count.saturating_add(array.length());
            if byte_count > 128 * 1024 * 1024 {
                return Err(error(Error::Limit("JS byte buffers".into())));
            }
            let copy = js_sys::Uint8Array::from(array.to_vec().as_slice());
            set(copy.as_ref())?;
            continue;
        }
        if seen.has(&v) {
            return Err(error(Error::Invalid("cyclic object graph".into())));
        }
        seen.add(&v);
        pending.push((v.clone(), parent.clone(), key.clone(), depth, true));
        if js_sys::Array::is_array(&v) {
            let length = js_sys::Reflect::get(&v, &JsValue::from_str("length"))
                .map_err(|_| error(Error::Invalid("array length".into())))?
                .as_f64()
                .ok_or_else(|| error(Error::Invalid("array length".into())))?;
            if !(0.0..=10_000.0).contains(&length) || length.fract() != 0.0 {
                return Err(error(Error::Limit("JS array entries".into())));
            }
            let length = length as u32;
            let copy: JsValue = js_sys::Array::new_with_length(length).into();
            set(&copy)?;
            for i in 0..length {
                let key = JsValue::from_f64(f64::from(i));
                let item = js_sys::Reflect::get(&v, &key)
                    .map_err(|_| error(Error::Invalid("throwing array entry".into())))?;
                pending.push((item, copy.clone(), key, depth.saturating_add(1), false));
            }
        } else {
            let keys = js_sys::Reflect::own_keys(&v)
                .map_err(|_| error(Error::Invalid("invalid JS object".into())))?;
            if keys.length() > 1024 {
                return Err(error(Error::Limit("JS object fields".into())));
            }
            let copy: JsValue =
                js_sys::Object::create(&JsValue::NULL.unchecked_into::<js_sys::Object>()).into();
            set(&copy)?;
            for key in keys.iter() {
                if !key.is_string() {
                    return Err(error(Error::Invalid("symbol keys".into())));
                }
                units = units.saturating_add(js_sys::JsString::from(key.clone()).length());
                if units > reprise::MAX_PAYLOAD_BYTES as u32 {
                    return Err(error(Error::Limit("JS key units".into())));
                }
                let item = js_sys::Reflect::get(&v, &key)
                    .map_err(|_| error(Error::Invalid("throwing property".into())))?;
                pending.push((item, copy.clone(), key, depth.saturating_add(1), false));
            }
        }
    }
    let snapshot = js_sys::Reflect::get(&holder, &JsValue::from_str("payload"))
        .map_err(|_| error(Error::Invalid("object snapshot".into())))?;
    serde_wasm_bindgen::from_value(snapshot).map_err(|e| error(Error::Invalid(e.to_string())))
}

#[wasm_bindgen(js_name=Workspace)]
pub struct WasmWorkspace {
    inner: reprise::Workspace,
}
#[wasm_bindgen(js_class=Workspace)]
impl WasmWorkspace {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            inner: reprise::Workspace::new(),
        }
    }
    pub fn create(&self, request: &CreatePayload) -> Result<WasmDocument, JsValue> {
        Ok(WasmDocument::new(
            self.inner
                .create(&decode(request.as_ref())?)
                .map_err(error)?,
        ))
    }
    pub fn open(
        &self,
        request: &OpenPayload,
        content: &js_sys::Uint8Array,
    ) -> Result<WasmDocument, JsValue> {
        let content = bytes(content, 128 * 1024 * 1024)?;
        Ok(WasmDocument::new(
            self.inner
                .open(&decode(request.as_ref())?, &content)
                .map_err(error)?,
        ))
    }
}
impl Default for WasmWorkspace {
    fn default() -> Self {
        Self::new()
    }
}
#[wasm_bindgen(js_name=DocumentSession)]
pub struct WasmDocument {
    inner: reprise::DocumentSession,
    jobs: BTreeMap<u32, reprise::LayoutJob>,
    plugins: BTreeMap<u32, reprise::Plugin>,
    next: u32,
}
impl WasmDocument {
    fn new(inner: reprise::DocumentSession) -> Self {
        Self {
            inner,
            jobs: BTreeMap::new(),
            plugins: BTreeMap::new(),
            next: 0,
        }
    }
    fn handle(&mut self) -> Result<u32, JsValue> {
        self.next = self
            .next
            .checked_add(1)
            .ok_or_else(|| error(Error::Limit("WASM handles".into())))?;
        Ok(self.next)
    }
}
#[wasm_bindgen(js_class=DocumentSession)]
impl WasmDocument {
    pub fn sync_export(&self, request: &SyncRequestPayload) -> Result<SyncPacketPayload, JsValue> {
        Ok(encode(self.inner.sync_export(&decode(request.as_ref())?))?.unchecked_into())
    }
    pub fn sync_import(
        &mut self,
        request: &SyncPacketPayload,
    ) -> Result<SyncReportPayload, JsValue> {
        Ok(encode(self.inner.sync_import(&decode(request.as_ref())?))?.unchecked_into())
    }
    pub fn set_selection(
        &mut self,
        request: &OptionalSelectionPayload,
    ) -> Result<OptionalStableSelectionPayload, JsValue> {
        Ok(encode(self.inner.set_selection(&decode(request.as_ref())?))?.unchecked_into())
    }
    pub fn anchor_selection(
        &self,
        request: &SelectionPayload,
    ) -> Result<StableSelectionPayload, JsValue> {
        Ok(encode(self.inner.anchor_selection(&decode(request.as_ref())?))?.unchecked_into())
    }
    pub fn resolve_selection(
        &self,
        request: &StableSelectionPayload,
    ) -> Result<OptionalSelectionPayload, JsValue> {
        Ok(encode(self.inner.resolve_selection(&decode(request.as_ref())?))?.unchecked_into())
    }
    pub fn presence(&self, request: &PresencePayload) -> Result<AwarenessPayload, JsValue> {
        Ok(encode(self.inner.presence(&decode(request.as_ref())?))?.unchecked_into())
    }
    pub fn resolve_presence(
        &self,
        request: &AwarenessPayload,
    ) -> Result<PresenceViewPayload, JsValue> {
        Ok(encode(self.inner.resolve_presence(&decode(request.as_ref())?))?.unchecked_into())
    }
    pub fn local_selection(&self) -> Result<OptionalSelectionPayload, JsValue> {
        Ok(encode(Ok(self.inner.local_selection()))?.unchecked_into())
    }
    pub fn undo_report(&mut self) -> Result<EditReportPayload, JsValue> {
        Ok(encode(self.inner.undo_report())?.unchecked_into())
    }
    pub fn redo_report(&mut self) -> Result<EditReportPayload, JsValue> {
        Ok(encode(self.inner.redo_report())?.unchecked_into())
    }
    pub fn copy_as(&self, request: &CopyAsPayload) -> Result<ExportedPayload, JsValue> {
        Ok(encode(self.inner.copy_as(&decode(request.as_ref())?))?.unchecked_into())
    }
    pub fn resources(&self) -> Result<ListResourcePayload, JsValue> {
        Ok(encode(self.inner.resources())?.unchecked_into())
    }
    pub fn resource_bytes(&self, request: &StringPayload) -> Result<BytesPayload, JsValue> {
        Ok(encode(self.inner.resource_bytes(&decode(request.as_ref())?))?.unchecked_into())
    }
    pub fn state(&self) -> Result<StatePayload, JsValue> {
        Ok(encode(self.inner.state())?.unchecked_into())
    }
    pub fn apply(&mut self, request: &TransactionPayload) -> Result<AppliedPayload, JsValue> {
        Ok(encode(self.inner.apply(&decode(request.as_ref())?))?.unchecked_into())
    }
    pub fn insert_image(
        &mut self,
        request: &ImageInsertPayload,
    ) -> Result<AppliedPayload, JsValue> {
        Ok(encode(self.inner.insert_image(&decode(request.as_ref())?))?.unchecked_into())
    }
    pub fn undo(&mut self) -> Result<BooleanPayload, JsValue> {
        Ok(encode(self.inner.undo())?.unchecked_into())
    }
    pub fn redo(&mut self) -> Result<BooleanPayload, JsValue> {
        Ok(encode(self.inner.redo())?.unchecked_into())
    }
    pub fn move_cursor(&self, request: &MovePayload) -> Result<CursorPayload, JsValue> {
        Ok(encode(self.inner.move_cursor(&decode(request.as_ref())?))?.unchecked_into())
    }
    pub fn caret_rect(&self, request: &CaretPayload) -> Result<PageRectPayload, JsValue> {
        Ok(encode(self.inner.caret_rect(&decode(request.as_ref())?))?.unchecked_into())
    }
    pub fn hit_test(&self, request: &HitTestPayload) -> Result<HitPayload, JsValue> {
        Ok(encode(self.inner.hit_test(&decode(request.as_ref())?))?.unchecked_into())
    }
    pub fn selection_rects(
        &self,
        request: &SelectionPayload,
    ) -> Result<ListPageRectPayload, JsValue> {
        Ok(encode(self.inner.selection_rects(&decode(request.as_ref())?))?.unchecked_into())
    }
    pub fn copy(&self, request: &SelectionPayload) -> Result<BytesPayload, JsValue> {
        Ok(encode(self.inner.copy(&decode(request.as_ref())?))?.unchecked_into())
    }
    pub fn import_text(&mut self, request: &TextImportPayload) -> Result<AppliedPayload, JsValue> {
        Ok(encode(self.inner.import_text(&decode(request.as_ref())?))?.unchecked_into())
    }
    pub fn save(&mut self) -> Result<BytesPayload, JsValue> {
        Ok(encode(self.inner.save())?.unchecked_into())
    }
    pub fn export(&self, request: &ExportFormatPayload) -> Result<ExportedPayload, JsValue> {
        Ok(encode(self.inner.export(&decode(request.as_ref())?))?.unchecked_into())
    }
    pub fn reading_order(&self) -> Result<ListReadingStepPayload, JsValue> {
        Ok(encode(self.inner.reading_order())?.unchecked_into())
    }
    pub fn export_updates(&self) -> Result<SyncUpdatePayload, JsValue> {
        Ok(encode(self.inner.export_updates())?.unchecked_into())
    }
    pub fn import_updates(
        &mut self,
        request: &SyncUpdatePayload,
    ) -> Result<SyncInfoPayload, JsValue> {
        Ok(encode(self.inner.import_updates(&decode(request.as_ref())?))?.unchecked_into())
    }
    pub fn validate_awareness(&self, request: &AwarenessPayload) -> Result<BytesPayload, JsValue> {
        Ok(encode(self.inner.validate_awareness(&decode(request.as_ref())?))?.unchecked_into())
    }

    pub fn diagnostics(&self) -> Result<ListDiagnosticPayload, JsValue> {
        Ok(encode(Ok(self.inner.diagnostics()))?.unchecked_into())
    }
    pub fn sync_info(&self) -> Result<SyncInfoPayload, JsValue> {
        Ok(encode(Ok(self.inner.sync_info()))?.unchecked_into())
    }
    pub fn declare_font(
        &mut self,
        request: &FontDeclarationPayload,
        content: &js_sys::Uint8Array,
    ) -> Result<FacePayload, JsValue> {
        let content = bytes(content, 32 * 1024 * 1024)?;
        Ok(encode(
            self.inner
                .declare_font(&decode(request.as_ref())?, &content),
        )?
        .unchecked_into())
    }
    pub fn register_asset(
        &mut self,
        request: &AssetDeclarationPayload,
        content: &js_sys::Uint8Array,
    ) -> Result<StringPayload, JsValue> {
        let content = bytes(content, reprise::MAX_ASSET_BYTES as u32)?;
        Ok(encode(
            self.inner
                .register_asset(&decode(request.as_ref())?, &content),
        )?
        .unchecked_into())
    }
    pub fn paste(
        &mut self,
        request: &PastePayload,
        content: &js_sys::Uint8Array,
    ) -> Result<AppliedPayload, JsValue> {
        let content = bytes(content, 96 * 1024 * 1024)?;
        Ok(encode(self.inner.paste(&decode(request.as_ref())?, &content))?.unchecked_into())
    }
    pub fn awareness(&self, content: &js_sys::Uint8Array) -> Result<AwarenessPayload, JsValue> {
        let content = bytes(content, reprise::MAX_AWARENESS_BYTES as u32)?;
        Ok(encode(self.inner.awareness(&content))?.unchecked_into())
    }
    pub fn start_layout(
        &mut self,
        request: &LayoutOptionsPayload,
    ) -> Result<HandlePayload, JsValue> {
        if self.jobs.len() >= 8 {
            return Err(error(Error::Limit("retained WASM jobs".into())));
        }
        let job = self
            .inner
            .start_layout(&decode(request.as_ref())?)
            .map_err(error)?;
        let handle = self.handle()?;
        self.jobs.insert(handle, job);
        Ok(encode(Ok(Payload::new(handle)))?.unchecked_into())
    }
    pub fn step(&mut self, job: u32, budget: u32) -> Result<LayoutProgressPayload, JsValue> {
        let job = self
            .jobs
            .get_mut(&job)
            .ok_or_else(|| error(Error::InvalidId(job.to_string())))?;
        Ok(encode(job.step(&mut self.inner, budget))?.unchecked_into())
    }
    pub fn cancel(&mut self, job: u32) -> Result<BooleanPayload, JsValue> {
        let job = self
            .jobs
            .get_mut(&job)
            .ok_or_else(|| error(Error::InvalidId(job.to_string())))?;
        job.cancel();
        Ok(encode(Ok(Payload::new(true)))?.unchecked_into())
    }
    pub fn release_job(&mut self, job: u32) -> Result<BooleanPayload, JsValue> {
        let removed = self.jobs.remove(&job).is_some();
        Ok(encode(Ok(Payload::new(removed)))?.unchecked_into())
    }
    pub fn partial_page(&self, job: u32, page: u32) -> Result<DisplayPagePayload, JsValue> {
        let job = self
            .jobs
            .get(&job)
            .ok_or_else(|| error(Error::InvalidId(job.to_string())))?;
        Ok(encode(job.display_page(&self.inner, page))?.unchecked_into())
    }
    pub fn display_page(&self, page: u32) -> Result<DisplayPagePayload, JsValue> {
        Ok(encode(self.inner.display_page(page))?.unchecked_into())
    }
    pub fn display_json(&self, page: u32) -> Result<StringPayload, JsValue> {
        Ok(encode(self.inner.display_json(page))?.unchecked_into())
    }
    pub fn svg(&self, page: u32) -> Result<StringPayload, JsValue> {
        Ok(encode(self.inner.svg(page))?.unchecked_into())
    }
    pub fn png(&self, page: u32, scale_permille: u32) -> Result<BytesPayload, JsValue> {
        Ok(encode(self.inner.png(page, scale_permille))?.unchecked_into())
    }
    pub fn load_plugin(
        &mut self,
        request: &PluginSpecPayload,
        content: &js_sys::Uint8Array,
    ) -> Result<HandlePayload, JsValue> {
        if self.plugins.len() >= 64 {
            return Err(error(Error::Limit("loaded WASM plugins".into())));
        }
        let content = bytes(content, 1_048_576)?;
        let plugin = self
            .inner
            .load_plugin(&decode(request.as_ref())?, &content)
            .map_err(error)?;
        let handle = self.handle()?;
        self.plugins.insert(handle, plugin);
        Ok(encode(Ok(Payload::new(handle)))?.unchecked_into())
    }
    pub fn install_plugin(
        &mut self,
        plugin: u32,
        request: &PluginInstallPayload,
    ) -> Result<BooleanPayload, JsValue> {
        let plugin = self
            .plugins
            .get(&plugin)
            .ok_or_else(|| error(Error::InvalidId(plugin.to_string())))?;
        Ok(encode(
            self.inner
                .install_plugin(plugin, &decode(request.as_ref())?),
        )?
        .unchecked_into())
    }
    pub fn release_plugin(&mut self, plugin: u32) -> Result<BooleanPayload, JsValue> {
        Ok(encode(Ok(Payload::new(self.plugins.remove(&plugin).is_some())))?.unchecked_into())
    }
    pub fn run_plugin_edit(
        &mut self,
        plugin: u32,
        request: &PluginEditPayload,
        content: &js_sys::Uint8Array,
    ) -> Result<AppliedPayload, JsValue> {
        let content = bytes(content, 1_048_576)?;
        let plugin = self
            .plugins
            .get(&plugin)
            .ok_or_else(|| error(Error::InvalidId(plugin.to_string())))?;
        Ok(encode(
            self.inner
                .run_plugin_edit(plugin, &decode(request.as_ref())?, &content),
        )?
        .unchecked_into())
    }
}
