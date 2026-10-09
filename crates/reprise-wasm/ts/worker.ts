/// <reference lib="webworker" />
// Compile as ES modules and deploy beside wasm-bindgen's web output.
import init, { Workspace, DocumentSession, setPanicHandler } from "./reprise_wasm.js";
import type { Payload, Create, Open, Transaction, LayoutOptions, LayoutProgress,
  DisplayPage, MarksPage, State, Bytes, SyncUpdate, ErrorPayload, FontDeclaration, AssetDeclaration, ImageInsert, SyncRequest, SyncPacket, SyncReport, SyncInfo,
  Selection, StableSelection, Presence, Awareness, PresenceView, EditReport, Applied } from "./types.js";

type Request = Payload<{ id: string } & (
  | { kind: "create"; request: Payload<Create> }
  | { kind: "open"; request: Payload<Open>; bytes: Uint8Array }
  | { kind: "font"; request: Payload<FontDeclaration>; bytes: Uint8Array }
  | { kind: "asset"; request: Payload<AssetDeclaration>; bytes: Uint8Array }
  | { kind: "image"; request: Payload<ImageInsert> }
  | { kind: "edit"; request: Payload<Transaction> }
  | { kind: "start"; request: Payload<LayoutOptions> }
  | { kind: "step"; job: number; budget: number }
  | { kind: "cancel"; job: number }
  | { kind: "marks"; page: number }
  | { kind: "save" }
  | { kind: "sync-export"; request?: Payload<SyncRequest> }
  | { kind: "sync-info" }
  | { kind: "sync-packet-import"; request: Payload<SyncPacket> }
  | { kind: "selection"; request: Payload<Selection | null> }
  | { kind: "resolve-selection"; request: Payload<StableSelection> }
  | { kind: "presence"; request: Payload<Presence> }
  | { kind: "resolve-presence"; request: Payload<Awareness> }
  | { kind: "undo" }
  | { kind: "redo" }
  | { kind: "sync-import"; request: Payload<SyncUpdate> }
  | { kind: "close" }
)>;
export type Response = Payload<{ id: string } & (
  | { kind: "state"; state: Payload<State> }
  | { kind: "edited"; applied: Payload<Applied>; state: Payload<State> }
  | { kind: "started"; job: number }
  | { kind: "layout"; job: number; progress: Payload<LayoutProgress>; pages: Payload<DisplayPage>[] }
  | { kind: "marks"; page: Payload<MarksPage> }
  | { kind: "saved"; content: Payload<Bytes> }
  | { kind: "sync"; update: Payload<SyncUpdate> }
  | { kind: "sync-packet"; packet: Payload<SyncPacket> }
  | { kind: "sync-info"; info: Payload<SyncInfo> }
  | { kind: "synced"; report: Payload<SyncReport>; state: Payload<State> }
  | { kind: "selection"; selection: Payload<Selection | null>; stable: Payload<StableSelection | null> }
  | { kind: "resolved-selection"; selection: Payload<Selection | null> }
  | { kind: "presence"; awareness: Payload<Awareness> }
  | { kind: "resolved-presence"; view: Payload<PresenceView> }
  | { kind: "history"; report: Payload<EditReport>; state: Payload<State> }
  | { kind: "asset"; hash: Payload<string> }
  | { kind: "ack" }
  | { kind: "error"; error: Payload<ErrorPayload> }
)>;
export type { Request };
const scope = self as DedicatedWorkerGlobalScope;
// Set by the engine's panic hook just before the instance traps. A panic=abort
// instance cannot recover: every later request fails with bindings.poisoned,
// and the host should terminate this worker, reopen the last saved package in
// a new one and resynchronise.
let panicked: string | undefined;
const ready = init().then(() => setPanicHandler((message: string) => { panicked = message; }));
function poisoned(id: string) {
  post({ id, kind: "error", error: { version: 1, data: { code: "bindings.poisoned", severity: "error", message: `engine panicked: ${panicked}`, command: null } } });
}
let workspace: Workspace | undefined;
let document: DocumentSession | undefined;
let active: number | undefined;
// Serialize requests, including initialization, to keep edit/job order explicit.
let queue = Promise.resolve();
function post(data: Response["data"], transfer: Transferable[] = []) {
  const response: Response = { version: 1, data };
  scope.postMessage(response, transfer);
}
function current(): DocumentSession { if (!document) throw new Error("Open a document first"); return document; }
function release() { if (active !== undefined && document) document.release_job(active); active = undefined; }
scope.onmessage = (event: MessageEvent<Request>) => {
  queue = queue.then(async () => {
    const request = event.data;
    const id = request?.data?.id ?? "";
    if (panicked !== undefined) { poisoned(id); return; }
    try {
      await ready;
      workspace ??= new Workspace();
      if (request.version !== 1) {
        post({ id, kind: "error", error: { version: 1, data: { code: "bindings.version", severity: "error", message: "Unsupported worker protocol", command: null } } }); return;
      }
      const data = request.data;
      switch (data.kind) {
        case "create": case "open":
          const candidate = data.kind === "create" ? workspace.create(data.request) : workspace.open(data.request, data.bytes);
          release(); document?.free(); document = candidate;
          post({ id, kind: "state", state: document.state() }); break;
        case "font": current().declare_font(data.request, data.bytes); release(); post({ id, kind: "ack" }); break;
        case "asset": {
          const hash = current().register_asset(data.request, data.bytes); release();
          post({ id, kind: "asset", hash }); break;
        }
        case "image": case "edit": {
          const applied = data.kind === "image" ? current().insert_image(data.request) : current().apply(data.request);
          release(); post({ id, kind: "edited", applied, state: current().state() }); break;
        }
        case "start": release(); active = current().start_layout(data.request).data; post({ id, kind: "started", job: active }); break;
        case "step": {
          if (data.job !== active) throw new Error("Obsolete job handle");
          const progress = current().step(data.job, data.budget);
          const pages = progress.data.pages.map(page => current().partial_page(data.job, page));
          post({ id, kind: "layout", job: data.job, progress, pages });
          if (progress.data.complete) release();
          break;
        }
        case "cancel": current().cancel(data.job); if (data.job === active) release(); post({ id, kind: "ack" }); break;
        case "marks": post({ id, kind: "marks", page: current().marks(data.page) }); break;
        case "save": { release(); const content = current().save(); post({ id, kind: "saved", content }, [content.data.bytes.buffer]); break; }
        case "sync-export": {
          if (data.request) {
            const packet = current().sync_export(data.request);
            post({ id, kind: "sync-packet", packet }, [packet.data.content.bytes.buffer]);
          } else {
            const update = current().export_updates();
            post({ id, kind: "sync", update }, [update.data.content.bytes.buffer]);
          }
          break;
        }
        case "sync-info": post({ id, kind: "sync-info", info: current().sync_info() }); break;
        case "sync-packet-import": {
          const report = current().sync_import(data.request);
          if (report.data.changed) release();
          post({ id, kind: "synced", report, state: current().state() }); break;
        }
        case "selection": {
          const stable = current().set_selection(data.request);
          post({ id, kind: "selection", selection: current().local_selection(), stable }); break;
        }
        case "resolve-selection": post({ id, kind: "resolved-selection", selection: current().resolve_selection(data.request) }); break;
        case "presence": {
          const awareness = current().presence(data.request);
          post({ id, kind: "presence", awareness }, [awareness.data.content.bytes.buffer]); break;
        }
        case "resolve-presence": post({ id, kind: "resolved-presence", view: current().resolve_presence(data.request) }); break;
        case "undo": case "redo": {
          const report = data.kind === "undo" ? current().undo_report() : current().redo_report();
          if (report.data.changed) release();
          post({ id, kind: "history", report, state: current().state() }); break;
        }
        case "sync-import": current().import_updates(data.request); release(); post({ id, kind: "state", state: current().state() }); break;
        case "close": release(); document?.free(); document = undefined; post({ id, kind: "ack" }); break;
        default: throw new Error("Unknown worker request");
      }
    } catch (cause) {
      if (panicked !== undefined) { document = undefined; workspace = undefined; poisoned(id); return; }
      const error = cause as Partial<Payload<ErrorPayload>>;
      post({ id, kind: "error", error: error?.version === 1 && error.data?.code ? error as Payload<ErrorPayload> : {
        version: 1, data: { code: "bindings.invalid", severity: "error", message: String(cause), command: null }
      } });
    }
  });
};
