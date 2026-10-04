//! Backend boundary: validation and calls are the conformance surface for a future accelerator.
use std::collections::BTreeSet;

use reprise_diag::Note;
use wasmi::TrapCode;
use wasmi::{
    Caller, CompilationMode, Config, Engine, ExternType, FuncType, Linker, Module, Store,
    StoreLimits, StoreLimitsBuilder, ValType,
};
use wasmparser::{Parser, Payload};

use crate::{
    ABI_VERSION, CallContext, CallResult, Capability, Envelope, Limits, Manifest, TextInsertion,
    codes,
};

pub(crate) trait Runtime: Send + Sync {
    fn call(
        &self,
        operation: u32,
        input: &[u8],
        context: &CallContext,
        envelope: &Envelope,
    ) -> Result<CallResult, Note>;
}

pub(crate) struct WasmiRuntime {
    engine: Engine,
    module: Module,
    imports: BTreeSet<Capability>,
}

fn invalid() -> Note {
    Note::warning(codes::INVALID, "invalid or unsupported core-WASM module")
}
fn limited() -> Note {
    Note::warning(codes::LIMIT, "plugin exceeded a sandbox resource limit")
}
fn bad_result() -> Note {
    Note::warning(
        codes::RESULT,
        "plugin returned invalid bytes, pointers or status",
    )
}
fn bad_abi() -> Note {
    Note::warning(codes::ABI, "plugin does not implement core-WASM ABI v1")
}

/// Inspect even unexported resources before compilation or instantiation.
fn preflight(bytes: &[u8], limits: &Limits) -> Result<(), Note> {
    if bytes.len() > 1_048_576 {
        return Err(limited());
    }
    for payload in Parser::new(0).parse_all(bytes) {
        match payload.map_err(|_| invalid())? {
            Payload::Version { encoding, .. } if encoding != wasmparser::Encoding::Module => {
                return Err(invalid());
            }
            Payload::StartSection { .. } => return Err(bad_abi()),
            Payload::MemorySection(reader) => {
                if reader.count() != 1 {
                    return Err(limited());
                }
                for memory in reader {
                    let m = memory.map_err(|_| invalid())?;
                    if m.shared || m.memory64 || m.page_size_log2.is_some() {
                        return Err(invalid());
                    }
                    if m.initial > u64::from(limits.memory_pages)
                        || m.maximum.is_none_or(|n| n > u64::from(limits.memory_pages))
                    {
                        return Err(limited());
                    }
                }
            }
            Payload::TableSection(reader) => {
                if reader.count() > 1 {
                    return Err(limited());
                }
                for table in reader {
                    let t = table.map_err(|_| invalid())?.ty;
                    if t.table64 || t.shared {
                        return Err(invalid());
                    }
                    if t.initial > u64::from(limits.table_elements)
                        || t.maximum
                            .is_none_or(|n| n > u64::from(limits.table_elements))
                    {
                        return Err(limited());
                    }
                }
            }
            Payload::TypeSection(r) if r.count() > 4096 => return Err(limited()),
            Payload::FunctionSection(r) if r.count() > 4096 => return Err(limited()),
            Payload::GlobalSection(r) if r.count() > 4096 => return Err(limited()),
            Payload::ExportSection(r) if r.count() > 4096 => return Err(limited()),
            Payload::ImportSection(r) if r.count() > 2 => return Err(limited()),
            Payload::CodeSectionEntry(body) => {
                let mut locals = 0u32;
                for local in body.get_locals_reader().map_err(|_| invalid())? {
                    locals = locals.saturating_add(local.map_err(|_| invalid())?.0);
                    if locals > 4096 {
                        return Err(limited());
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

impl WasmiRuntime {
    pub(crate) fn load(bytes: &[u8], manifest: &Manifest, limits: &Limits) -> Result<Self, Note> {
        preflight(bytes, limits)?;
        let mut config = Config::default();
        config
            .consume_fuel(true)
            .compilation_mode(CompilationMode::Eager)
            .wasm_multi_memory(false)
            .wasm_custom_page_sizes(false)
            .set_max_recursion_depth(128)
            .set_max_stack_height(65_536)
            .set_max_cached_stacks(0);
        // Neither the SIMD nor threads features are compiled into this backend.
        // The `deterministic` Cargo feature canonicalises arithmetic NaNs.
        let engine = Engine::new(&config);
        let module = Module::new(&engine, bytes).map_err(|_| invalid())?;
        for import in module.imports() {
            let cap = manifest
                .imports
                .iter()
                .find(|c| import.module() == "reprise_v1" && import.name() == c.import());
            let Some(cap) = cap else {
                return Err(Note::warning(
                    codes::CAPABILITY,
                    "plugin requests an undeclared or unknown import",
                ));
            };
            let expected = match cap {
                Capability::ReadText => FuncType::new([ValType::I32; 3], [ValType::I32]),
                Capability::InsertText => FuncType::new([ValType::I32; 4], [ValType::I32]),
            };
            if !matches!(import.ty(), ExternType::Func(f) if *f == expected) {
                return Err(bad_abi());
            }
        }
        for (name, arity) in [
            ("reprise_abi_version", 0),
            ("reprise_alloc", 1),
            ("reprise_call", 5),
        ] {
            let expected = FuncType::new(vec![ValType::I32; arity], [ValType::I32]);
            if !matches!(module.get_export(name), Some(ExternType::Func(f)) if f == expected) {
                return Err(bad_abi());
            }
        }
        if !matches!(module.get_export("memory"), Some(ExternType::Memory(_))) {
            return Err(bad_abi());
        }
        Ok(Self {
            engine,
            module,
            imports: manifest.imports.clone(),
        })
    }
}

struct HostState {
    limits: StoreLimits,
    context: CallContext,
    insertions: Vec<TextInsertion>,
    staged_bytes: usize,
    buffer_limit: usize,
    failure: Option<Note>,
}

fn fail(caller: &mut Caller<'_, HostState>, note: Note) -> wasmi::Error {
    if caller.data().failure.is_none() {
        caller.data_mut().failure = Some(note);
    }
    wasmi::Error::new("reprise host refusal")
}

fn charge(caller: &mut Caller<'_, HostState>, bytes: usize) -> Result<(), wasmi::Error> {
    let cost = 32u64.saturating_add(bytes as u64);
    let remaining = caller.get_fuel()?;
    let Some(next) = remaining.checked_sub(cost) else {
        return Err(fail(
            caller,
            Note::warning(codes::FUEL, "plugin exhausted its per-call fuel budget"),
        ));
    };
    caller.set_fuel(next)
}

fn read_text(
    mut caller: Caller<'_, HostState>,
    handle: i32,
    pointer: i32,
    capacity: i32,
) -> Result<i32, wasmi::Error> {
    charge(&mut caller, 0)?;
    let Some(text) = caller.data().context.texts.get(&(handle as u32)) else {
        return Ok(-1);
    };
    let len = text.len();
    if capacity < 0 || len > capacity as usize {
        return Ok(-2);
    }
    if pointer < 0 {
        return Err(fail(&mut caller, bad_result()));
    }
    charge(&mut caller, len)?;
    let Some(memory) = caller.get_export("memory").and_then(|e| e.into_memory()) else {
        return Err(fail(&mut caller, bad_abi()));
    };
    let text = caller
        .data()
        .context
        .texts
        .get(&(handle as u32))
        .map(|s| s.as_bytes().to_vec())
        .ok_or_else(|| wasmi::Error::new("missing immutable text"))?;
    memory
        .write(&mut caller, pointer as usize, &text)
        .map_err(|_| fail(&mut caller, bad_result()))?;
    Ok(len as i32)
}

fn insert_text(
    mut caller: Caller<'_, HostState>,
    handle: i32,
    at: i32,
    pointer: i32,
    len: i32,
) -> Result<i32, wasmi::Error> {
    charge(&mut caller, 0)?;
    if !caller.data().context.editable.contains(&(handle as u32)) {
        return Err(fail(
            &mut caller,
            Note::warning(codes::CAPABILITY, "editing handle was not granted"),
        ));
    }
    if at < 0 || pointer < 0 || len < 0 {
        return Err(fail(&mut caller, bad_result()));
    }
    let len = len as usize;
    let state = caller.data();
    if state.insertions.len() >= 256 || state.staged_bytes.saturating_add(len) > state.buffer_limit
    {
        return Err(fail(&mut caller, limited()));
    }
    charge(&mut caller, len)?;
    let Some(memory) = caller.get_export("memory").and_then(|e| e.into_memory()) else {
        return Err(fail(&mut caller, bad_abi()));
    };
    let mut bytes = vec![0; len];
    memory
        .read(&caller, pointer as usize, &mut bytes)
        .map_err(|_| fail(&mut caller, bad_result()))?;
    let text = String::from_utf8(bytes).map_err(|_| fail(&mut caller, bad_result()))?;
    let state = caller.data_mut();
    state.staged_bytes = state.staged_bytes.saturating_add(len);
    state.insertions.push(TextInsertion {
        handle: handle as u32,
        at: at as u32,
        text,
    });
    Ok(0)
}

fn execution_error(error: wasmi::Error, state: &HostState) -> Note {
    if let Some(note) = &state.failure {
        return note.clone();
    }
    match error.as_trap_code() {
        Some(TrapCode::OutOfFuel) => {
            Note::warning(codes::FUEL, "plugin exhausted its per-call fuel budget")
        }
        Some(
            TrapCode::GrowthOperationLimited
            | TrapCode::StackOverflow
            | TrapCode::OutOfSystemMemory,
        ) => limited(),
        _ => Note::warning(codes::TRAP, "plugin trapped; extension fallback used"),
    }
}

impl Runtime for WasmiRuntime {
    fn call(
        &self,
        operation: u32,
        input: &[u8],
        context: &CallContext,
        envelope: &Envelope,
    ) -> Result<CallResult, Note> {
        let limit = envelope.limits.buffer_bytes as usize;
        let text_bytes = context
            .texts
            .values()
            .try_fold(0usize, |n, s| n.checked_add(s.len()))
            .ok_or_else(limited)?;
        if input.len() > limit
            || text_bytes > limit
            || context.texts.len() > 256
            || context.editable.len() > 256
        {
            return Err(limited());
        }
        let limits = StoreLimitsBuilder::new()
            .memory_size(envelope.limits.memory_pages as usize * 65_536)
            .table_elements(envelope.limits.table_elements as usize)
            .instances(1)
            .memories(1)
            .tables(1)
            .trap_on_grow_failure(true)
            .build();
        let mut store = Store::new(
            &self.engine,
            HostState {
                limits,
                context: context.clone(),
                insertions: Vec::new(),
                staged_bytes: 0,
                buffer_limit: limit,
                failure: None,
            },
        );
        store.limiter(|s| &mut s.limits);
        store
            .set_fuel(envelope.limits.fuel)
            .map_err(|_| limited())?;
        let mut linker = Linker::new(&self.engine);
        if self.imports.contains(&Capability::ReadText) {
            linker
                .func_wrap("reprise_v1", "read_text", read_text)
                .map_err(|_| bad_abi())?;
        }
        if self.imports.contains(&Capability::InsertText) {
            linker
                .func_wrap("reprise_v1", "insert_text", insert_text)
                .map_err(|_| bad_abi())?;
        }
        let instance = linker
            .instantiate_and_start(&mut store, &self.module)
            .map_err(|e| execution_error(e, store.data()))?;
        let version = instance
            .get_typed_func::<(), i32>(&store, "reprise_abi_version")
            .map_err(|_| bad_abi())?;
        if version
            .call(&mut store, ())
            .map_err(|e| execution_error(e, store.data()))?
            != ABI_VERSION as i32
        {
            return Err(bad_abi());
        }
        let alloc = instance
            .get_typed_func::<i32, i32>(&store, "reprise_alloc")
            .map_err(|_| bad_abi())?;
        let input_ptr = alloc
            .call(&mut store, input.len() as i32)
            .map_err(|e| execution_error(e, store.data()))?;
        let output_ptr = alloc
            .call(&mut store, limit as i32)
            .map_err(|e| execution_error(e, store.data()))?;
        if input_ptr < 0 || output_ptr < 0 {
            return Err(bad_result());
        }
        let input_start = input_ptr as usize;
        let output_start = output_ptr as usize;
        let input_end = input_start
            .checked_add(input.len())
            .ok_or_else(bad_result)?;
        let output_end = output_start.checked_add(limit).ok_or_else(bad_result)?;
        if input_start < output_end && output_start < input_end {
            return Err(bad_result());
        }
        let memory = instance.get_memory(&store, "memory").ok_or_else(bad_abi)?;
        if output_end > memory.data(&store).len() {
            return Err(bad_result());
        }
        memory
            .write(&mut store, input_start, input)
            .map_err(|_| bad_result())?;
        // Initialised output prevents allocator data from masquerading as a result.
        memory
            .write(&mut store, output_start, &vec![0; limit])
            .map_err(|_| bad_result())?;
        let call = instance
            .get_typed_func::<(i32, i32, i32, i32, i32), i32>(&store, "reprise_call")
            .map_err(|_| bad_abi())?;
        let len = call
            .call(
                &mut store,
                (
                    operation as i32,
                    input_ptr,
                    input.len() as i32,
                    output_ptr,
                    limit as i32,
                ),
            )
            .map_err(|e| execution_error(e, store.data()))?;
        if len == -3 {
            return Err(limited());
        }
        if len == -4 {
            return Err(Note::warning(
                codes::CAPABILITY,
                "plugin refused the operation's authority",
            ));
        }
        if len < 0 || len as usize > limit {
            return Err(bad_result());
        }
        let mut bytes = vec![0; len as usize];
        memory
            .read(&store, output_start, &mut bytes)
            .map_err(|_| bad_result())?;
        let fuel_used = envelope
            .limits
            .fuel
            .saturating_sub(store.get_fuel().map_err(|_| limited())?);
        let insertions = std::mem::take(&mut store.data_mut().insertions);
        Ok(CallResult {
            bytes,
            fuel_used,
            insertions,
        })
    }
}
