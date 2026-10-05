/// <reference lib="webworker" />
// Compile as ES modules and deploy beside wasm-bindgen's web output.
import init, { Workspace, DocumentSession } from "./reprise_wasm.js";
import type { Payload, Create, Open, Transaction, LayoutOptions, LayoutProgress,
  DisplayPage, State, Bytes, SyncUpdate, ErrorPayload, FontDeclaration, AssetDeclaration, ImageInsert } from "./types.js";

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
  | { kind: "save" }
  | { kind: "sync-export" }
  | { kind: "sync-import"; request: Payload<SyncUpdate> }
  | { kind: "close" }
)>;
export type Response = Payload<{ id: string } & (
  | { kind: "state"; state: Payload<State> }
  | { kind: "started"; job: number }
  | { kind: "layout"; job: number; progress: Payload<LayoutProgress>; pages: Payload<DisplayPage>[] }
  | { kind: "saved"; content: Payload<Bytes> }
  | { kind: "sync"; update: Payload<SyncUpdate> }
  | { kind: "ack" }
  | { kind: "error"; error: Payload<ErrorPayload> }
)>;
export type { Request };
const scope = self as DedicatedWorkerGlobalScope;
const ready = init();
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
        case "asset": current().register_asset(data.request, data.bytes); release(); post({ id, kind: "ack" }); break;
        case "image": current().insert_image(data.request); release(); post({ id, kind: "state", state: current().state() }); break;
        case "edit": current().apply(data.request); release(); post({ id, kind: "state", state: current().state() }); break;
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
        case "save": { release(); const content = current().save(); post({ id, kind: "saved", content }, [content.data.bytes.buffer]); break; }
        case "sync-export": { const update = current().export_updates(); post({ id, kind: "sync", update }, [update.data.content.bytes.buffer]); break; }
        case "sync-import": current().import_updates(data.request); release(); post({ id, kind: "state", state: current().state() }); break;
        case "close": release(); document?.free(); document = undefined; post({ id, kind: "ack" }); break;
        default: throw new Error("Unknown worker request");
      }
    } catch (cause) {
      const error = cause as Partial<Payload<ErrorPayload>>;
      post({ id, kind: "error", error: error?.version === 1 && error.data?.code ? error as Payload<ErrorPayload> : {
        version: 1, data: { code: "bindings.invalid", severity: "error", message: String(cause), command: null }
      } });
    }
  });
};
