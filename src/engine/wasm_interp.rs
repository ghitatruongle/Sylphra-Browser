use crate::wasm::{FuncType, ValueType, WasmModule};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WasmValue {
    I32(i32),
    I64(i64),
    F32(f32),
    F64(f64),
}

pub const MAX_INTERPRETER_STEPS: u64 = 1_000_000;

pub const MAX_MEMORY_BYTES: usize = 16 * 1024 * 1024;

pub const MAX_CALL_DEPTH_WASM: usize = 256;
pub const MAX_STACK_VALUES: usize = 10_000;
pub const MAX_LABELS: usize = 1_024;

const PAGE_SIZE: usize = 65_536;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ControlKind {
    Block,
    Loop,
    If,
}

#[derive(Debug, Clone)]
struct ControlFrame {
    kind: ControlKind,

    else_pc: usize,

    end_pc: usize,

    stack_height: usize,

    result_count: usize,
}

struct Frame {
    function_index: u32,
    pc: usize,
    locals: Vec<WasmValue>,
    stack: Vec<WasmValue>,
    controls: Vec<ControlFrame>,

    stack_height: usize,
    steps: u64,
}

#[derive(Debug)]
pub struct WasmInstance {
    module: WasmModule,
    globals: Vec<WasmValue>,
    memory: Vec<u8>,
    table: Vec<Option<u32>>,
}

impl WasmInstance {
    pub fn memory(&self) -> &[u8] {
        &self.memory
    }

    pub fn write_memory(&mut self, offset: usize, bytes: &[u8]) -> Result<(), String> {
        let end = offset
            .checked_add(bytes.len())
            .ok_or_else(|| "Memory write offset overflow".to_string())?;
        if end > self.memory.len() || self.memory.len() > MAX_MEMORY_BYTES {
            return Err("Memory write out of bounds".to_string());
        }
        self.memory[offset..end].copy_from_slice(bytes);
        Ok(())
    }

    pub fn read_memory(&self, offset: usize, len: usize) -> Result<&[u8], String> {
        let end = offset
            .checked_add(len)
            .ok_or_else(|| "Memory read offset overflow".to_string())?;
        self.memory
            .get(offset..end)
            .ok_or_else(|| "Memory read out of bounds".to_string())
    }

    pub fn grow_memory(&mut self, delta_pages: u32) -> Result<u32, String> {
        let old_pages = (self.memory.len() / PAGE_SIZE) as u32;
        let delta_bytes = (delta_pages as usize)
            .checked_mul(PAGE_SIZE)
            .ok_or_else(|| "Memory growth overflow".to_string())?;
        let new_len = self
            .memory
            .len()
            .checked_add(delta_bytes)
            .ok_or_else(|| "Memory growth overflow".to_string())?;
        if new_len > MAX_MEMORY_BYTES {
            return Err("Memory growth exceeds 16 MiB".to_string());
        }
        self.memory.resize(new_len, 0);
        Ok(old_pages)
    }

    pub fn memory_bytes(&self) -> usize {
        self.memory.len()
    }

    pub fn table(&self) -> &[Option<u32>] {
        &self.table
    }

    pub fn imported_function_count(&self) -> usize {
        self.module
            .imports
            .iter()
            .filter(|import| matches!(import.kind, crate::wasm::ImportKind::Function(_)))
            .count()
    }

    fn defined_body_index(&self, function_index: u32) -> Option<usize> {
        let imported = self.imported_function_count();
        let idx = function_index as usize;
        if idx < imported {
            return None;
        }
        Some(idx - imported)
    }

    pub fn function_type(&self, function_index: u32) -> Option<&FuncType> {
        let idx = function_index as usize;
        let imported = self.imported_function_count();
        if idx < imported {
            let mut seen = 0usize;
            for import in &self.module.imports {
                if let crate::wasm::ImportKind::Function(type_index) = import.kind {
                    if seen == idx {
                        return self.module.types.get(type_index as usize);
                    }
                    seen += 1;
                }
            }
            return None;
        }
        let defined = idx - imported;
        let type_index = self.module.function_type_indices.get(defined)?;
        self.module.types.get(*type_index as usize)
    }

    pub fn exported_function(&self, name: &str) -> Option<u32> {
        self.module.exports.iter().find_map(|export| {
            (export.name == name && matches!(export.kind, crate::wasm::ExportKind::Function))
                .then_some(export.index)
        })
    }
}

impl WasmInstance {
    pub fn instantiate(module: WasmModule) -> Result<Self, String> {
        let memory_pages = module
            .memories
            .first()
            .map(|memory| memory.limits.min)
            .unwrap_or(0);
        let memory_bytes = (memory_pages as usize)
            .checked_mul(PAGE_SIZE)
            .ok_or_else(|| "Memory size overflow".to_string())?;
        if memory_bytes > MAX_MEMORY_BYTES {
            return Err("Memory exceeds 16 MiB".to_string());
        }
        let mut table = Vec::new();
        if let Some(table_type) = module.tables.first() {
            let entries = table_type.limits.min as usize;
            if entries > crate::wasm::MAX_TABLE_ENTRIES {
                return Err("Table exceeds budget".to_string());
            }
            table = vec![None; entries];
        }
        let mut instance = Self {
            module,
            globals: Vec::new(),
            memory: vec![0; memory_bytes],
            table,
        };

        for index in 0..instance.module.globals.len() {
            let value = instance.eval_const_expr(&instance.module.globals[index].init)?;
            instance.globals.push(value);
        }

        for segment in &instance.module.elements.clone() {
            let base = instance.eval_const_expr(&segment.offset)?;
            let base = match base {
                WasmValue::I32(value) if value >= 0 => value as usize,
                _ => return Err("Element offset must be a non-negative i32".to_string()),
            };
            for (offset, function_index) in segment.function_indices.iter().enumerate() {
                if function_index >= &instance.module.total_functions {
                    return Err("Element function index out of range".to_string());
                }
                let slot = base + offset;
                if slot >= instance.table.len() {
                    return Err("Element segment exceeds table size".to_string());
                }
                instance.table[slot] = Some(*function_index);
            }
        }

        for segment in &instance.module.data.clone() {
            let base = instance.eval_const_expr(&segment.offset)?;
            let base = match base {
                WasmValue::I32(value) if value >= 0 => value as usize,
                _ => return Err("Data offset must be a non-negative i32".to_string()),
            };
            let end = base
                .checked_add(segment.bytes.len())
                .ok_or_else(|| "Data segment offset overflow".to_string())?;
            if end > instance.memory.len() {
                return Err("Data segment exceeds memory".to_string());
            }
            instance.memory[base..end].copy_from_slice(&segment.bytes);
        }
        Ok(instance)
    }

    fn eval_const_expr(&self, expr: &[u8]) -> Result<WasmValue, String> {
        let mut cursor = crate::wasm::Cursor::new(expr);
        let mut value = None;
        while !cursor.is_empty() {
            let opcode = cursor.read_u8()?;
            match opcode {
                0x41 => value = Some(WasmValue::I32(cursor.read_leb_i32()?)),
                0x42 => value = Some(WasmValue::I64(cursor.read_leb_i64()?)),
                0x43 => {
                    let bytes = cursor.read_bytes(4)?;
                    let arr: [u8; 4] = bytes.try_into().map_err(|_| {
                        "Constant expression produced invalid f32 bytes".to_string()
                    })?;
                    value = Some(WasmValue::F32(f32::from_le_bytes(arr)));
                }
                0x44 => {
                    let bytes = cursor.read_bytes(8)?;
                    let arr: [u8; 8] = bytes.try_into().map_err(|_| {
                        "Constant expression produced invalid f64 bytes".to_string()
                    })?;
                    value = Some(WasmValue::F64(f64::from_le_bytes(arr)));
                }
                0x23 => {
                    let index = cursor.read_leb_u32()? as usize;
                    value = Some(
                        *self
                            .globals
                            .get(index)
                            .ok_or_else(|| "Global index out of range".to_string())?,
                    );
                }
                0x0B => break,
                other => return Err(format!("Unsupported const opcode 0x{other:02X}")),
            }
        }
        value.ok_or_else(|| "Constant expression produced no value".to_string())
    }

    pub fn invoke(
        &mut self,
        function_index: u32,
        args: &[WasmValue],
    ) -> Result<Vec<WasmValue>, String> {
        let defined = self.defined_body_index(function_index).ok_or_else(|| {
            "Cannot invoke an imported function (host imports unsupported)".to_string()
        })?;
        let type_index = self
            .module
            .function_type_indices
            .get(defined)
            .copied()
            .ok_or_else(|| "Function index out of range or is an import".to_string())?;
        let func_type = self
            .module
            .types
            .get(type_index as usize)
            .cloned()
            .ok_or_else(|| "Function type index out of range".to_string())?;
        if args.len() != func_type.parameters.len() {
            return Err("Argument count does not match function type".to_string());
        }
        if self.module.bodies.get(defined).is_none() {
            return Err(
                "Cannot invoke an imported function (host imports unsupported)".to_string(),
            );
        }
        let mut frames: Vec<Frame> = Vec::new();
        let mut total_steps: u64 = 0;
        let results = self.run_function(function_index, args, &mut frames, &mut total_steps)?;
        if results.len() != func_type.results.len() {
            return Err("Result count does not match function type".to_string());
        }
        Ok(results)
    }

    fn run_function(
        &mut self,
        function_index: u32,
        args: &[WasmValue],
        frames: &mut Vec<Frame>,
        total_steps: &mut u64,
    ) -> Result<Vec<WasmValue>, String> {
        if frames.len() >= MAX_CALL_DEPTH_WASM {
            return Err("WASM call depth exceeded".to_string());
        }
        let defined = self
            .defined_body_index(function_index)
            .ok_or_else(|| "Function has no body (import)".to_string())?;
        let body = self
            .module
            .bodies
            .get(defined)
            .ok_or_else(|| "Function has no body (import)".to_string())?
            .clone();
        let mut locals = args.to_vec();
        for decl in &body.locals {
            for _ in 0..decl.count {
                locals.push(match decl.value_type {
                    ValueType::I32 => WasmValue::I32(0),
                    ValueType::I64 => WasmValue::I64(0),
                    ValueType::F32 => WasmValue::F32(0.0),
                    ValueType::F64 => WasmValue::F64(0.0),
                    _ => return Err("Non-numeric local type".to_string()),
                });
            }
        }
        frames.push(Frame {
            function_index,
            pc: 0,
            locals,
            stack: Vec::new(),
            controls: Vec::new(),
            stack_height: 0,
            steps: 0,
        });
        self.execute_frame(frames, total_steps)
    }

    fn execute_frame(
        &mut self,
        frames: &mut Vec<Frame>,
        total_steps: &mut u64,
    ) -> Result<Vec<WasmValue>, String> {
        loop {
            let frame = frames
                .last_mut()
                .ok_or_else(|| "InvalidStateError: WASM frame is detached".to_string())?;
            frame.steps += 1;
            *total_steps += 1;
            if *total_steps > MAX_INTERPRETER_STEPS {
                return Err("WASM step budget exceeded".to_string());
            }
            if frame.stack.len() > MAX_STACK_VALUES {
                return Err("WASM operand stack budget exceeded".to_string());
            }
            let defined = self
                .defined_body_index(frame.function_index)
                .ok_or_else(|| "Function has no body (import)".to_string())?;

            let code = self
                .module
                .bodies
                .get(defined)
                .ok_or_else(|| "Function has no body (import)".to_string())?
                .code
                .clone();
            if frame.pc >= code.len() {
                return Err("Function ran past its body".to_string());
            }
            let opcode = code[frame.pc];
            frame.pc += 1;
            if opcode == 0x0B {
                let frame = frames
                    .last_mut()
                    .ok_or_else(|| "InvalidStateError: WASM frame is detached".to_string())?;
                if frame.controls.is_empty() {
                    let results = frame.stack[frame.stack_height..].to_vec();
                    frames.pop();
                    return Ok(results);
                }
                let control = frame.controls.pop().ok_or_else(|| {
                    "InvalidStateError: WASM control stack is detached".to_string()
                })?;
                let base = control.stack_height;
                let above = frame.stack.split_off(base);
                let result_count = control.result_count.min(above.len());
                let keep = above[above.len() - result_count..].to_vec();
                frame.stack.extend(keep);

                continue;
            }
            let action = self.exec_instruction(opcode, &code, frames)?;
            match action {
                Action::Continue => {}
                Action::Branch(label) => {
                    let frame = frames
                        .last_mut()
                        .ok_or_else(|| "InvalidStateError: WASM frame is detached".to_string())?;
                    let label = label as usize;
                    if label == frame.controls.len() {
                        let results = frame.stack[frame.stack_height..].to_vec();
                        frames.pop();
                        return Ok(results);
                    }
                    let control_index = frame
                        .controls
                        .len()
                        .checked_sub(1 + label)
                        .filter(|v| *v < frame.controls.len())
                        .ok_or_else(|| "br label out of range".to_string())?;
                    let control = frame.controls[control_index].clone();
                    let is_loop = control.kind == ControlKind::Loop;
                    let target = if is_loop {
                        control.else_pc
                    } else {
                        control.end_pc
                    };

                    let keep_count = control
                        .result_count
                        .min(frame.stack.len().saturating_sub(control.stack_height));
                    let keep: Vec<_> = frame.stack[frame.stack.len() - keep_count..].to_vec();
                    frame.stack.truncate(control.stack_height);
                    frame.stack.extend(keep);

                    frame.controls.truncate(control_index + 1);
                    frame.pc = target;
                }
                Action::Call(index) => {
                    let param_count = self
                        .function_type(index)
                        .ok_or_else(|| "Function index out of range".to_string())?
                        .parameters
                        .len();
                    let frame = frames
                        .last_mut()
                        .ok_or_else(|| "InvalidStateError: WASM frame is detached".to_string())?;
                    if frame.stack.len() < param_count {
                        return Err("call missing arguments on stack".to_string());
                    }
                    let split_at = frame.stack.len() - param_count;
                    let callee_args = frame.stack.split_off(split_at);
                    let callee_results =
                        self.run_function(index, &callee_args, frames, total_steps)?;
                    frames
                        .last_mut()
                        .ok_or_else(|| "InvalidStateError: WASM frame is detached".to_string())?
                        .stack
                        .extend(callee_results);
                }
                Action::CallIndirect(type_index) => {
                    let frame = frames
                        .last_mut()
                        .ok_or_else(|| "InvalidStateError: WASM frame is detached".to_string())?;
                    let table_value = frame
                        .stack
                        .pop()
                        .ok_or_else(|| "call_indirect missing table index".to_string())?;
                    let WasmValue::I32(index) = table_value else {
                        return Err("call_indirect table index must be i32".to_string());
                    };
                    if index < 0 {
                        return Err("call_indirect negative index".to_string());
                    }
                    let entry = self
                        .table
                        .get(index as usize)
                        .and_then(|entry| *entry)
                        .ok_or_else(|| "call_indirect null or out of range".to_string())?;
                    let callee_defined = self
                        .defined_body_index(entry)
                        .ok_or_else(|| "call_indirect target is an import".to_string())?;
                    let callee_type_index = self
                        .module
                        .function_type_indices
                        .get(callee_defined)
                        .copied()
                        .ok_or_else(|| "call_indirect target is an import".to_string())?;
                    if callee_type_index != type_index {
                        return Err("call_indirect type mismatch".to_string());
                    }
                    let func_type = self
                        .module
                        .types
                        .get(type_index as usize)
                        .ok_or_else(|| "call_indirect type index out of range".to_string())?;
                    let param_count = func_type.parameters.len();
                    if frame.stack.len() < param_count {
                        return Err("call_indirect missing arguments on stack".to_string());
                    }
                    let split_at = frame.stack.len() - param_count;
                    let callee_args = frame.stack.split_off(split_at);
                    let callee_results =
                        self.run_function(entry, &callee_args, frames, total_steps)?;
                    frames
                        .last_mut()
                        .ok_or_else(|| "InvalidStateError: WASM frame is detached".to_string())?
                        .stack
                        .extend(callee_results);
                }
                Action::Return => {
                    let frame = frames
                        .last()
                        .ok_or_else(|| "InvalidStateError: WASM frame is detached".to_string())?;
                    let results = frame.stack[frame.stack_height..].to_vec();
                    frames.pop();
                    return Ok(results);
                }
            }
        }
    }

    fn exec_instruction(
        &mut self,
        opcode: u8,
        code: &[u8],
        frames: &mut [Frame],
    ) -> Result<Action, String> {
        let read_leb_u32 = |frame: &mut Frame| -> Result<u32, String> {
            let mut result = 0u32;
            let mut shift = 0u32;
            for _ in 0..5 {
                let byte = *code
                    .get(frame.pc)
                    .ok_or_else(|| "Instruction immediate out of range".to_string())?;
                frame.pc += 1;
                result |= u32::from(byte & 0x7F) << shift;
                if byte & 0x80 == 0 {
                    return Ok(result);
                }
                shift += 7;
            }
            Err("Instruction immediate too long".to_string())
        };
        let read_leb_i32 = |frame: &mut Frame| -> Result<i32, String> {
            let mut result = 0i32;
            let mut shift = 0u32;
            let mut byte;
            loop {
                byte = *code
                    .get(frame.pc)
                    .ok_or_else(|| "Instruction immediate out of range".to_string())?;
                frame.pc += 1;
                result |= i32::from(byte & 0x7F) << shift;
                shift += 7;
                if byte & 0x80 == 0 {
                    break;
                }
                if shift >= 35 {
                    return Err("Instruction immediate too long".to_string());
                }
            }
            if shift < 32 && byte & 0x40 != 0 {
                result |= -1i32 << shift;
            }
            Ok(result)
        };
        let read_leb_i64 = |frame: &mut Frame| -> Result<i64, String> {
            let mut result = 0i64;
            let mut shift = 0u32;
            let mut byte;
            loop {
                byte = *code
                    .get(frame.pc)
                    .ok_or_else(|| "Instruction immediate out of range".to_string())?;
                frame.pc += 1;
                result |= i64::from(byte & 0x7F) << shift;
                shift += 7;
                if byte & 0x80 == 0 {
                    break;
                }
                if shift >= 70 {
                    return Err("Instruction immediate too long".to_string());
                }
            }
            if shift < 64 && byte & 0x40 != 0 {
                result |= -1i64 << shift;
            }
            Ok(result)
        };

        let frame = frames
            .last_mut()
            .ok_or_else(|| "InvalidStateError: WASM frame is detached".to_string())?;
        match opcode {
            0x02..=0x04 => {
                let block_type = *code
                    .get(frame.pc)
                    .ok_or_else(|| "Missing block type".to_string())?;
                frame.pc += 1;
                if !matches!(block_type, 0x40 | 0x7F | 0x7E | 0x7D | 0x7C) {
                    let mut tmp = 0u32;
                    let mut shift = 0u32;
                    loop {
                        let byte = *code
                            .get(frame.pc)
                            .ok_or_else(|| "Block type index out of range".to_string())?;
                        frame.pc += 1;
                        tmp |= u32::from(byte & 0x7F) << shift;
                        if byte & 0x80 == 0 {
                            break;
                        }
                        shift += 7;
                    }
                    let _ = tmp;
                }
                let kind = match opcode {
                    0x02 => ControlKind::Block,
                    0x03 => ControlKind::Loop,
                    _ => ControlKind::If,
                };
                let result_count = match block_type {
                    0x40 => 0,
                    0x7C..=0x7F => 1,
                    type_index => {
                        let types = &self.module.types;
                        let type_index = type_index as usize;
                        if type_index >= types.len() {
                            return Err("Block type index out of range".to_string());
                        }
                        types[type_index].results.len().min(1)
                    }
                };
                if kind == ControlKind::If {
                    let cond = frame
                        .stack
                        .pop()
                        .ok_or_else(|| "if missing condition".to_string())?;
                    let WasmValue::I32(condition) = cond else {
                        return Err("if condition must be i32".to_string());
                    };
                    if condition == 0 {
                        let else_pc = find_else_or_end(code, frame.pc)?;
                        let end_pc = find_end(code, frame.pc)?;
                        frame.pc = if else_pc < end_pc { else_pc } else { end_pc };
                        frame.controls.push(ControlFrame {
                            kind,
                            else_pc,
                            end_pc,
                            stack_height: frame.stack.len(),
                            result_count,
                        });
                        return Ok(Action::Continue);
                    }
                }

                let end_pc = find_end(code, frame.pc)?;
                frame.controls.push(ControlFrame {
                    kind,
                    else_pc: frame.pc,
                    end_pc,
                    stack_height: frame.stack.len(),
                    result_count,
                });
                Ok(Action::Continue)
            }
            0x05 => {
                let end_pc = find_end(code, frame.pc)?;
                frame.pc = end_pc;
                Ok(Action::Continue)
            }
            0x0C | 0x0D => {
                let label = read_leb_u32(frame)?;
                if label as usize >= frame.controls.len().saturating_add(1) {
                    return Err("br label out of range".to_string());
                }
                if opcode == 0x0D {
                    let condition = frame
                        .stack
                        .pop()
                        .ok_or_else(|| "br_if missing condition".to_string())?;
                    let WasmValue::I32(condition) = condition else {
                        return Err("br_if condition must be i32".to_string());
                    };
                    if condition == 0 {
                        return Ok(Action::Continue);
                    }
                }
                Ok(Action::Branch(label))
            }
            0x0E => {
                let count = read_leb_u32(frame)?;
                if count > MAX_LABELS as u32 {
                    return Err("br_table label budget exceeded".to_string());
                }
                let mut labels = Vec::with_capacity(count as usize);
                for _ in 0..count {
                    labels.push(read_leb_u32(frame)?);
                }
                let default_label = read_leb_u32(frame)?;
                let index_value = frame
                    .stack
                    .pop()
                    .ok_or_else(|| "br_table missing index".to_string())?;
                let WasmValue::I32(index) = index_value else {
                    return Err("br_table index must be i32".to_string());
                };
                let label = if index >= 0 && (index as usize) < labels.len() {
                    labels[index as usize]
                } else {
                    default_label
                };
                Ok(Action::Branch(label))
            }
            0x0F => Ok(Action::Return),
            0x10 => {
                let index = read_leb_u32(frame)?;
                Ok(Action::Call(index))
            }
            0x11 => {
                let type_index = read_leb_u32(frame)?;

                let reserved = *code
                    .get(frame.pc)
                    .ok_or_else(|| "call_indirect reserved byte missing".to_string())?;
                frame.pc += 1;
                if reserved != 0 {
                    return Err("call_indirect reserved byte must be zero".to_string());
                }
                Ok(Action::CallIndirect(type_index))
            }
            0x12 => {
                let index = read_leb_u32(frame)?;
                Ok(Action::Call(index))
            }
            0x13 => {
                let index = read_leb_u32(frame)?;
                Ok(Action::CallIndirect(index))
            }

            0x1A => {
                frame.stack.pop().ok_or("drop on empty stack")?;
                Ok(Action::Continue)
            }
            0x1B => {
                let frame = frames
                    .last_mut()
                    .ok_or_else(|| "InvalidStateError: WASM frame is detached".to_string())?;
                let condition = frame
                    .stack
                    .pop()
                    .ok_or_else(|| "select missing condition".to_string())?;
                let WasmValue::I32(condition) = condition else {
                    return Err("select condition must be i32".to_string());
                };
                let second = frame
                    .stack
                    .pop()
                    .ok_or_else(|| "select missing operand".to_string())?;
                let first = frame
                    .stack
                    .pop()
                    .ok_or_else(|| "select missing operand".to_string())?;
                if std::mem::discriminant(&first) != std::mem::discriminant(&second) {
                    return Err("select operand type mismatch".to_string());
                }
                frame
                    .stack
                    .push(if condition != 0 { first } else { second });
                Ok(Action::Continue)
            }

            0x20 => {
                let index = read_leb_u32(frame)?;
                let value = *frame
                    .locals
                    .get(index as usize)
                    .ok_or_else(|| "local.get index out of range".to_string())?;
                frame.stack.push(value);
                Ok(Action::Continue)
            }
            0x21 => {
                let index = read_leb_u32(frame)?;
                let value = frame
                    .stack
                    .pop()
                    .ok_or_else(|| "local.set on empty stack".to_string())?;
                let slot = frame
                    .locals
                    .get_mut(index as usize)
                    .ok_or_else(|| "local.set index out of range".to_string())?;
                *slot = value;
                Ok(Action::Continue)
            }
            0x22 => {
                let index = read_leb_u32(frame)?;
                let value = *frame
                    .stack
                    .last()
                    .ok_or_else(|| "local.tee on empty stack".to_string())?;
                let slot = frame
                    .locals
                    .get_mut(index as usize)
                    .ok_or_else(|| "local.tee index out of range".to_string())?;
                *slot = value;
                Ok(Action::Continue)
            }
            0x23 => {
                let index = read_leb_u32(frame)?;
                let value = *self
                    .globals
                    .get(index as usize)
                    .ok_or_else(|| "global.get index out of range".to_string())?;
                frame.stack.push(value);
                Ok(Action::Continue)
            }
            0x24 => {
                let index = read_leb_u32(frame)?;
                let value = frame
                    .stack
                    .pop()
                    .ok_or_else(|| "global.set on empty stack".to_string())?;
                let slot = self
                    .globals
                    .get_mut(index as usize)
                    .ok_or_else(|| "global.set index out of range".to_string())?;
                *slot = value;
                Ok(Action::Continue)
            }

            0x28..=0x35 => {
                let align = read_leb_u32(frame)?;
                let offset = read_leb_u32(frame)?;
                let _ = align;
                let address = self.effective_address(frame, offset as usize)?;
                let frame = frames
                    .last_mut()
                    .ok_or_else(|| "InvalidStateError: WASM frame is detached".to_string())?;
                let value = self.memory_load(address, opcode)?;
                frame.stack.push(value);
                Ok(Action::Continue)
            }
            0x36..=0x3E => {
                let align = read_leb_u32(frame)?;
                let offset = read_leb_u32(frame)?;
                let _ = align;
                let frame = frames
                    .last_mut()
                    .ok_or_else(|| "InvalidStateError: WASM frame is detached".to_string())?;
                let value = frame
                    .stack
                    .pop()
                    .ok_or_else(|| "store on empty stack".to_string())?;
                let address = self.effective_address(frame, offset as usize)?;
                self.memory_store(address, opcode, value)?;
                Ok(Action::Continue)
            }
            0x3F => {
                let reserved = *code
                    .get(frame.pc)
                    .ok_or_else(|| "memory.size reserved byte missing".to_string())?;
                frame.pc += 1;
                if reserved != 0 {
                    return Err("memory.size reserved byte must be zero".to_string());
                }
                let pages = (self.memory.len() / PAGE_SIZE) as i32;
                frame.stack.push(WasmValue::I32(pages));
                Ok(Action::Continue)
            }
            0x40 => {
                let reserved = *code
                    .get(frame.pc)
                    .ok_or_else(|| "memory.grow reserved byte missing".to_string())?;
                frame.pc += 1;
                if reserved != 0 {
                    return Err("memory.grow reserved byte must be zero".to_string());
                }
                let delta = frame
                    .stack
                    .pop()
                    .ok_or_else(|| "memory.grow missing delta".to_string())?;
                let WasmValue::I32(delta) = delta else {
                    return Err("memory.grow delta must be i32".to_string());
                };
                let old_pages = (self.memory.len() / PAGE_SIZE) as i32;
                if delta < 0 {
                    frame.stack.push(WasmValue::I32(-1));
                    return Ok(Action::Continue);
                }
                let new_bytes = self
                    .memory
                    .len()
                    .checked_add((delta as usize).saturating_mul(PAGE_SIZE))
                    .ok_or_else(|| "memory.grow overflow".to_string())?;
                if new_bytes > MAX_MEMORY_BYTES {
                    frame.stack.push(WasmValue::I32(-1));
                    return Ok(Action::Continue);
                }
                self.memory.resize(new_bytes, 0);
                frame.stack.push(WasmValue::I32(old_pages));
                Ok(Action::Continue)
            }

            0x41 => {
                let value = read_leb_i32(frame)?;
                frame.stack.push(WasmValue::I32(value));
                Ok(Action::Continue)
            }
            0x42 => {
                let value = read_leb_i64(frame)?;
                frame.stack.push(WasmValue::I64(value));
                Ok(Action::Continue)
            }
            0x43 => {
                let bytes = code
                    .get(frame.pc..frame.pc + 4)
                    .ok_or_else(|| "f32.const missing bytes".to_string())?;
                frame.pc += 4;
                let arr: [u8; 4] = bytes
                    .try_into()
                    .map_err(|_| "f32.const has invalid byte length".to_string())?;
                frame.stack.push(WasmValue::F32(f32::from_le_bytes(arr)));
                Ok(Action::Continue)
            }
            0x44 => {
                let bytes = code
                    .get(frame.pc..frame.pc + 8)
                    .ok_or_else(|| "f64.const missing bytes".to_string())?;
                frame.pc += 8;
                let arr: [u8; 8] = bytes
                    .try_into()
                    .map_err(|_| "f64.const has invalid byte length".to_string())?;
                frame.stack.push(WasmValue::F64(f64::from_le_bytes(arr)));
                Ok(Action::Continue)
            }

            0x45..=0xC4 => {
                self.exec_numeric(opcode, frames)?;
                Ok(Action::Continue)
            }
            other => Err(format!("Unsupported opcode 0x{other:02X}")),
        }
    }

    fn effective_address(&self, frame: &mut Frame, offset: usize) -> Result<usize, String> {
        let base = frame
            .stack
            .pop()
            .ok_or_else(|| "memory access missing address".to_string())?;
        let WasmValue::I32(base) = base else {
            return Err("memory address must be i32".to_string());
        };
        if base < 0 {
            return Err("negative memory address".to_string());
        }
        (base as usize)
            .checked_add(offset)
            .ok_or_else(|| "memory address overflow".to_string())
    }

    fn memory_load(&self, address: usize, opcode: u8) -> Result<WasmValue, String> {
        let read = |size: usize| -> Result<u64, String> {
            let end = address
                .checked_add(size)
                .ok_or_else(|| "memory access out of range".to_string())?;
            let slice = self
                .memory
                .get(address..end)
                .ok_or_else(|| "memory access out of range".to_string())?;
            let mut bytes = [0u8; 8];
            bytes[..size].copy_from_slice(slice);
            Ok(u64::from_le_bytes(bytes))
        };
        match opcode {
            0x28 => Ok(WasmValue::I32(read(4)? as u32 as i32)),
            0x29 => Ok(WasmValue::I64(read(8)? as i64)),
            0x2A => Ok(WasmValue::F32(f32::from_bits(read(4)? as u32))),
            0x2B => Ok(WasmValue::F64(f64::from_bits(read(8)?))),
            0x2C => Ok(WasmValue::I32(read(1)? as u8 as i8 as i32)),
            0x2D => Ok(WasmValue::I32(read(1)? as u8 as i32)),
            0x2E => Ok(WasmValue::I32(read(2)? as u16 as i16 as i32)),
            0x2F => Ok(WasmValue::I32(read(2)? as u16 as i32)),
            0x30 => Ok(WasmValue::I64(read(1)? as u8 as i8 as i64)),
            0x31 => Ok(WasmValue::I64(read(1)? as u8 as i64)),
            0x32 => Ok(WasmValue::I64(read(2)? as u16 as i16 as i64)),
            0x33 => Ok(WasmValue::I64(read(2)? as u16 as i64)),
            0x34 => Ok(WasmValue::I64(read(4)? as u32 as i32 as i64)),
            0x35 => Ok(WasmValue::I64(read(4)? as u32 as i64)),
            other => Err(format!("Unknown load opcode 0x{other:02X}")),
        }
    }

    fn memory_store(&mut self, address: usize, opcode: u8, value: WasmValue) -> Result<(), String> {
        let (size, bytes) = match (opcode, value) {
            (0x36, WasmValue::I32(v)) => (4, (v as u32 as u64).to_le_bytes()),
            (0x37, WasmValue::I64(v)) => (8, (v as u64).to_le_bytes()),
            (0x38, WasmValue::F32(v)) => (4, (v.to_bits() as u64).to_le_bytes()),
            (0x39, WasmValue::F64(v)) => (8, v.to_bits().to_le_bytes()),
            (0x3A, WasmValue::I32(v)) => (1, ((v as u8) as u64).to_le_bytes()),
            (0x3B, WasmValue::I32(v)) => (2, ((v as u16) as u64).to_le_bytes()),
            (0x3C, WasmValue::I64(v)) => (1, ((v as u8) as u64).to_le_bytes()),
            (0x3D, WasmValue::I64(v)) => (2, ((v as u16) as u64).to_le_bytes()),
            (0x3E, WasmValue::I64(v)) => (4, ((v as u32) as u64).to_le_bytes()),
            (opcode, value) => {
                return Err(format!(
                    "Store opcode 0x{opcode:02X} with incompatible value {:?}",
                    value
                ))
            }
        };
        let end = address
            .checked_add(size)
            .ok_or_else(|| "memory access out of range".to_string())?;
        let slice = self
            .memory
            .get_mut(address..end)
            .ok_or_else(|| "memory access out of range".to_string())?;
        slice.copy_from_slice(&bytes[..size]);
        Ok(())
    }

    fn exec_numeric(&mut self, opcode: u8, frames: &mut [Frame]) -> Result<(), String> {
        let frame = frames
            .last_mut()
            .ok_or_else(|| "InvalidStateError: WASM frame is detached".to_string())?;
        let mut pop = |frame: &mut Frame| -> Result<WasmValue, String> {
            frame
                .stack
                .pop()
                .ok_or_else(|| "numeric op on empty stack".to_string())
        };
        let push = |frame: &mut Frame, value: WasmValue| {
            frame.stack.push(value);
        };
        match opcode {
            0x45 => {
                let a = pop_i32(&mut pop, frame)?;
                push(&mut *frame, WasmValue::I32(i32::from(a == 0)));
            }
            0x46 => {
                let (b, a) = (pop_i32(&mut pop, frame)?, pop_i32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a == b)));
            }
            0x47 => {
                let (b, a) = (pop_i32(&mut pop, frame)?, pop_i32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a != b)));
            }
            0x48 => {
                let (b, a) = (pop_i32(&mut pop, frame)?, pop_i32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a < b)));
            }
            0x49 => {
                let (b, a) = (pop_u32(&mut pop, frame)?, pop_u32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a < b)));
            }
            0x4A => {
                let (b, a) = (pop_i32(&mut pop, frame)?, pop_i32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a > b)));
            }
            0x4B => {
                let (b, a) = (pop_u32(&mut pop, frame)?, pop_u32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a > b)));
            }
            0x4C => {
                let (b, a) = (pop_i32(&mut pop, frame)?, pop_i32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a <= b)));
            }
            0x4D => {
                let (b, a) = (pop_u32(&mut pop, frame)?, pop_u32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a <= b)));
            }
            0x4E => {
                let (b, a) = (pop_i32(&mut pop, frame)?, pop_i32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a >= b)));
            }
            0x4F => {
                let (b, a) = (pop_u32(&mut pop, frame)?, pop_u32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a >= b)));
            }

            0x50 => {
                let a = pop_i64(&mut pop, frame)?;
                push(&mut *frame, WasmValue::I32(i32::from(a == 0)));
            }
            0x51 => {
                let (b, a) = (pop_i64(&mut pop, frame)?, pop_i64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a == b)));
            }
            0x52 => {
                let (b, a) = (pop_i64(&mut pop, frame)?, pop_i64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a != b)));
            }
            0x53 => {
                let (b, a) = (pop_i64(&mut pop, frame)?, pop_i64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a < b)));
            }
            0x54 => {
                let (b, a) = (pop_u64(&mut pop, frame)?, pop_u64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a < b)));
            }
            0x55 => {
                let (b, a) = (pop_i64(&mut pop, frame)?, pop_i64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a > b)));
            }
            0x56 => {
                let (b, a) = (pop_u64(&mut pop, frame)?, pop_u64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a > b)));
            }
            0x57 => {
                let (b, a) = (pop_i64(&mut pop, frame)?, pop_i64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a <= b)));
            }
            0x58 => {
                let (b, a) = (pop_u64(&mut pop, frame)?, pop_u64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a <= b)));
            }
            0x59 => {
                let (b, a) = (pop_i64(&mut pop, frame)?, pop_i64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a >= b)));
            }
            0x5A => {
                let (b, a) = (pop_u64(&mut pop, frame)?, pop_u64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a >= b)));
            }

            0x5B => {
                let (b, a) = (pop_f32(&mut pop, frame)?, pop_f32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a == b)));
            }
            0x5C => {
                let (b, a) = (pop_f32(&mut pop, frame)?, pop_f32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a != b)));
            }
            0x5D => {
                let (b, a) = (pop_f32(&mut pop, frame)?, pop_f32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a < b)));
            }
            0x5E => {
                let (b, a) = (pop_f32(&mut pop, frame)?, pop_f32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a > b)));
            }
            0x5F => {
                let (b, a) = (pop_f32(&mut pop, frame)?, pop_f32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a <= b)));
            }
            0x60 => {
                let (b, a) = (pop_f32(&mut pop, frame)?, pop_f32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a >= b)));
            }

            0x61 => {
                let (b, a) = (pop_f64(&mut pop, frame)?, pop_f64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a == b)));
            }
            0x62 => {
                let (b, a) = (pop_f64(&mut pop, frame)?, pop_f64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a != b)));
            }
            0x63 => {
                let (b, a) = (pop_f64(&mut pop, frame)?, pop_f64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a < b)));
            }
            0x64 => {
                let (b, a) = (pop_f64(&mut pop, frame)?, pop_f64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a > b)));
            }
            0x65 => {
                let (b, a) = (pop_f64(&mut pop, frame)?, pop_f64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a <= b)));
            }
            0x66 => {
                let (b, a) = (pop_f64(&mut pop, frame)?, pop_f64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a >= b)));
            }

            0x67 => {
                let a = pop_i32(&mut pop, frame)?;
                push(&mut *frame, WasmValue::I32(a.leading_zeros() as i32));
            }
            0x68 => {
                let a = pop_i32(&mut pop, frame)?;
                push(&mut *frame, WasmValue::I32(a.trailing_zeros() as i32));
            }
            0x69 => {
                let a = pop_i32(&mut pop, frame)?;
                push(&mut *frame, WasmValue::I32(a.count_ones() as i32));
            }
            0x6A => {
                let (b, a) = (pop_i32(&mut pop, frame)?, pop_i32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(a.wrapping_add(b)));
            }
            0x6B => {
                let (b, a) = (pop_i32(&mut pop, frame)?, pop_i32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(a.wrapping_sub(b)));
            }
            0x6C => {
                let (b, a) = (pop_i32(&mut pop, frame)?, pop_i32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(a.wrapping_mul(b)));
            }
            0x6D => {
                let (b, a) = (pop_i32(&mut pop, frame)?, pop_i32(&mut pop, frame)?);
                if b == 0 {
                    return Err("integer divide by zero".to_string());
                }
                if a == i32::MIN && b == -1 {
                    return Err("integer overflow".to_string());
                }
                push(&mut *frame, WasmValue::I32(a.wrapping_div(b)));
            }
            0x6E => {
                let (b, a) = (pop_u32(&mut pop, frame)?, pop_u32(&mut pop, frame)?);
                if b == 0 {
                    return Err("integer divide by zero".to_string());
                }
                push(&mut *frame, WasmValue::I32((a / b) as i32));
            }
            0x6F => {
                let (b, a) = (pop_i32(&mut pop, frame)?, pop_i32(&mut pop, frame)?);
                if b == 0 {
                    return Err("integer divide by zero".to_string());
                }
                push(&mut *frame, WasmValue::I32(a.wrapping_rem(b)));
            }
            0x70 => {
                let (b, a) = (pop_u32(&mut pop, frame)?, pop_u32(&mut pop, frame)?);
                if b == 0 {
                    return Err("integer divide by zero".to_string());
                }
                push(&mut *frame, WasmValue::I32((a % b) as i32));
            }
            0x71 => {
                let (b, a) = (pop_i32(&mut pop, frame)?, pop_i32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(a & b));
            }
            0x72 => {
                let (b, a) = (pop_i32(&mut pop, frame)?, pop_i32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(a | b));
            }
            0x73 => {
                let (b, a) = (pop_i32(&mut pop, frame)?, pop_i32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(a ^ b));
            }
            0x74 => {
                let (b, a) = (pop_i32(&mut pop, frame)?, pop_i32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(a << (b as u32 & 31)));
            }
            0x75 => {
                let (b, a) = (pop_i32(&mut pop, frame)?, pop_i32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(a >> (b as u32 & 31)));
            }
            0x76 => {
                let (b, a) = (pop_u32(&mut pop, frame)?, pop_u32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32((a >> (b & 31)) as i32));
            }
            0x77 => {
                let (b, a) = (pop_i32(&mut pop, frame)?, pop_i32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(a.rotate_left(b as u32 & 31)));
            }
            0x78 => {
                let (b, a) = (pop_i32(&mut pop, frame)?, pop_i32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(a.rotate_right(b as u32 & 31)));
            }

            0x79 => {
                let a = pop_i64(&mut pop, frame)?;
                push(&mut *frame, WasmValue::I64(a.leading_zeros() as i64));
            }
            0x7A => {
                let a = pop_i64(&mut pop, frame)?;
                push(&mut *frame, WasmValue::I64(a.trailing_zeros() as i64));
            }
            0x7B => {
                let a = pop_i64(&mut pop, frame)?;
                push(&mut *frame, WasmValue::I64(a.count_ones() as i64));
            }
            0x7C => {
                let (b, a) = (pop_i64(&mut pop, frame)?, pop_i64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I64(a.wrapping_add(b)));
            }
            0x7D => {
                let (b, a) = (pop_i64(&mut pop, frame)?, pop_i64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I64(a.wrapping_sub(b)));
            }
            0x7E => {
                let (b, a) = (pop_i64(&mut pop, frame)?, pop_i64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I64(a.wrapping_mul(b)));
            }
            0x7F => {
                let (b, a) = (pop_i64(&mut pop, frame)?, pop_i64(&mut pop, frame)?);
                if b == 0 {
                    return Err("integer divide by zero".to_string());
                }
                if a == i64::MIN && b == -1 {
                    return Err("integer overflow".to_string());
                }
                push(&mut *frame, WasmValue::I64(a.wrapping_div(b)));
            }
            0x80 => {
                let (b, a) = (pop_u64(&mut pop, frame)?, pop_u64(&mut pop, frame)?);
                if b == 0 {
                    return Err("integer divide by zero".to_string());
                }
                push(&mut *frame, WasmValue::I64((a / b) as i64));
            }
            0x81 => {
                let (b, a) = (pop_i64(&mut pop, frame)?, pop_i64(&mut pop, frame)?);
                if b == 0 {
                    return Err("integer divide by zero".to_string());
                }
                push(&mut *frame, WasmValue::I64(a.wrapping_rem(b)));
            }
            0x82 => {
                let (b, a) = (pop_u64(&mut pop, frame)?, pop_u64(&mut pop, frame)?);
                if b == 0 {
                    return Err("integer divide by zero".to_string());
                }
                push(&mut *frame, WasmValue::I64((a % b) as i64));
            }
            0x83 => {
                let (b, a) = (pop_i64(&mut pop, frame)?, pop_i64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I64(a & b));
            }
            0x84 => {
                let (b, a) = (pop_i64(&mut pop, frame)?, pop_i64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I64(a | b));
            }
            0x85 => {
                let (b, a) = (pop_i64(&mut pop, frame)?, pop_i64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I64(a ^ b));
            }
            0x86 => {
                let (b, a) = (pop_i64(&mut pop, frame)?, pop_i64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I64(a << (b as u64 & 63)));
            }
            0x87 => {
                let (b, a) = (pop_i64(&mut pop, frame)?, pop_i64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I64(a >> (b as u64 & 63)));
            }
            0x88 => {
                let (b, a) = (pop_u64(&mut pop, frame)?, pop_u64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I64((a >> (b & 63)) as i64));
            }
            0x89 => {
                let (b, a) = (pop_i64(&mut pop, frame)?, pop_i64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I64(a.rotate_left(b as u32 & 63)));
            }
            0x8A => {
                let (b, a) = (pop_i64(&mut pop, frame)?, pop_i64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I64(a.rotate_right(b as u32 & 63)));
            }

            0x8B => {
                let a = pop_f32(&mut pop, frame)?;
                push(&mut *frame, WasmValue::F32(a.abs()));
            }
            0x8C => {
                let a = pop_f32(&mut pop, frame)?;
                push(&mut *frame, WasmValue::F32(-a));
            }
            0x8D => {
                let a = pop_f32(&mut pop, frame)?;
                push(&mut *frame, WasmValue::F32(a.ceil()));
            }
            0x8E => {
                let a = pop_f32(&mut pop, frame)?;
                push(&mut *frame, WasmValue::F32(a.floor()));
            }
            0x8F => {
                let a = pop_f32(&mut pop, frame)?;
                push(&mut *frame, WasmValue::F32(a.trunc()));
            }
            0x90 => {
                let a = pop_f32(&mut pop, frame)?;
                push(&mut *frame, WasmValue::F32(a.round_ties_even()));
            }
            0x91 => {
                let a = pop_f32(&mut pop, frame)?;
                push(&mut *frame, WasmValue::F32(a.sqrt()));
            }
            0x92 => {
                let (b, a) = (pop_f32(&mut pop, frame)?, pop_f32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::F32(a + b));
            }
            0x93 => {
                let (b, a) = (pop_f32(&mut pop, frame)?, pop_f32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::F32(a - b));
            }
            0x94 => {
                let (b, a) = (pop_f32(&mut pop, frame)?, pop_f32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::F32(a * b));
            }
            0x95 => {
                let (b, a) = (pop_f32(&mut pop, frame)?, pop_f32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::F32(a / b));
            }
            0x96 => {
                let (b, a) = (pop_f32(&mut pop, frame)?, pop_f32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::F32(wasm_min_f32(a, b)));
            }
            0x97 => {
                let (b, a) = (pop_f32(&mut pop, frame)?, pop_f32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::F32(wasm_max_f32(a, b)));
            }
            0x98 => {
                let (b, a) = (pop_f32(&mut pop, frame)?, pop_f32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::F32(a.copysign(b)));
            }

            0x99 => {
                let a = pop_f64(&mut pop, frame)?;
                push(&mut *frame, WasmValue::F64(a.abs()));
            }
            0x9A => {
                let a = pop_f64(&mut pop, frame)?;
                push(&mut *frame, WasmValue::F64(-a));
            }
            0x9B => {
                let a = pop_f64(&mut pop, frame)?;
                push(&mut *frame, WasmValue::F64(a.ceil()));
            }
            0x9C => {
                let a = pop_f64(&mut pop, frame)?;
                push(&mut *frame, WasmValue::F64(a.floor()));
            }
            0x9D => {
                let a = pop_f64(&mut pop, frame)?;
                push(&mut *frame, WasmValue::F64(a.trunc()));
            }
            0x9E => {
                let a = pop_f64(&mut pop, frame)?;
                push(&mut *frame, WasmValue::F64(a.round_ties_even()));
            }
            0x9F => {
                let a = pop_f64(&mut pop, frame)?;
                push(&mut *frame, WasmValue::F64(a.sqrt()));
            }
            0xA0 => {
                let (b, a) = (pop_f64(&mut pop, frame)?, pop_f64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::F64(a + b));
            }
            0xA1 => {
                let (b, a) = (pop_f64(&mut pop, frame)?, pop_f64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::F64(a - b));
            }
            0xA2 => {
                let (b, a) = (pop_f64(&mut pop, frame)?, pop_f64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::F64(a * b));
            }
            0xA3 => {
                let (b, a) = (pop_f64(&mut pop, frame)?, pop_f64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::F64(a / b));
            }
            0xA4 => {
                let (b, a) = (pop_f64(&mut pop, frame)?, pop_f64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::F64(wasm_min_f64(a, b)));
            }
            0xA5 => {
                let (b, a) = (pop_f64(&mut pop, frame)?, pop_f64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::F64(wasm_max_f64(a, b)));
            }
            0xA6 => {
                let (b, a) = (pop_f64(&mut pop, frame)?, pop_f64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::F64(a.copysign(b)));
            }

            0xA7 => {
                let a = pop_i64(&mut pop, frame)?;
                push(&mut *frame, WasmValue::I32(a as i32));
            }
            0xA8 => {
                let a = pop_f32(&mut pop, frame)?;
                push(&mut *frame, WasmValue::I32(trunc_f64_to_i32(a as f64)?));
            }
            0xA9 => {
                let a = pop_f32(&mut pop, frame)?;
                push(
                    &mut *frame,
                    WasmValue::I32(trunc_f64_to_u32(a as f64)? as i32),
                );
            }
            0xAA => {
                let a = pop_f64(&mut pop, frame)?;
                push(&mut *frame, WasmValue::I32(trunc_f64_to_i32(a)?));
            }
            0xAB => {
                let a = pop_f64(&mut pop, frame)?;
                push(&mut *frame, WasmValue::I32(trunc_f64_to_u32(a)? as i32));
            }
            0xAC => {
                let a = pop_i32(&mut pop, frame)?;
                push(&mut *frame, WasmValue::I64(a as i64));
            }
            0xAD => {
                let a = pop_i32(&mut pop, frame)?;
                push(&mut *frame, WasmValue::I64(a as u32 as u64 as i64));
            }
            0xAE => {
                let a = pop_f32(&mut pop, frame)?;
                push(&mut *frame, WasmValue::I64(trunc_f64_to_i64(a as f64)?));
            }
            0xAF => {
                let a = pop_f32(&mut pop, frame)?;
                push(
                    &mut *frame,
                    WasmValue::I64(trunc_f64_to_u64(a as f64)? as i64),
                );
            }
            0xB0 => {
                let a = pop_f64(&mut pop, frame)?;
                push(&mut *frame, WasmValue::I64(trunc_f64_to_i64(a)?));
            }
            0xB1 => {
                let a = pop_f64(&mut pop, frame)?;
                push(&mut *frame, WasmValue::I64(trunc_f64_to_u64(a)? as i64));
            }
            0xB2 => {
                let a = pop_i32(&mut pop, frame)?;
                push(&mut *frame, WasmValue::F32(a as f32));
            }
            0xB3 => {
                let a = pop_i32(&mut pop, frame)?;
                push(&mut *frame, WasmValue::F32(a as u32 as f32));
            }
            0xB4 => {
                let a = pop_i64(&mut pop, frame)?;
                push(&mut *frame, WasmValue::F32(a as f32));
            }
            0xB5 => {
                let a = pop_i64(&mut pop, frame)?;
                push(&mut *frame, WasmValue::F32(a as u64 as f32));
            }
            0xB6 => {
                let a = pop_f64(&mut pop, frame)?;
                push(&mut *frame, WasmValue::F32(a as f32));
            }
            0xB7 => {
                let a = pop_i32(&mut pop, frame)?;
                push(&mut *frame, WasmValue::F64(a as f64));
            }
            0xB8 => {
                let a = pop_i32(&mut pop, frame)?;
                push(&mut *frame, WasmValue::F64(a as u32 as f64));
            }
            0xB9 => {
                let a = pop_i64(&mut pop, frame)?;
                push(&mut *frame, WasmValue::F64(a as f64));
            }
            0xBA => {
                let a = pop_i64(&mut pop, frame)?;
                push(&mut *frame, WasmValue::F64(a as u64 as f64));
            }
            0xBB => {
                let a = pop_f32(&mut pop, frame)?;
                push(&mut *frame, WasmValue::F64(a as f64));
            }
            0xBC => {
                let a = pop_f32(&mut pop, frame)?;
                push(&mut *frame, WasmValue::I32(a.to_bits() as i32));
            }
            0xBD => {
                let a = pop_f64(&mut pop, frame)?;
                push(&mut *frame, WasmValue::I64(a.to_bits() as i64));
            }
            0xBE => {
                let a = pop_i32(&mut pop, frame)?;
                push(&mut *frame, WasmValue::F32(f32::from_bits(a as u32)));
            }
            0xBF => {
                let a = pop_i64(&mut pop, frame)?;
                push(&mut *frame, WasmValue::F64(f64::from_bits(a as u64)));
            }
            0xC0 => {
                let a = pop_i32(&mut pop, frame)?;
                push(&mut *frame, WasmValue::I32(a as i8 as i32));
            }
            0xC1 => {
                let a = pop_i32(&mut pop, frame)?;
                push(&mut *frame, WasmValue::I32(a as i16 as i32));
            }
            0xC2 => {
                let a = pop_i64(&mut pop, frame)?;
                push(&mut *frame, WasmValue::I64(a as i8 as i64));
            }
            0xC3 => {
                let a = pop_i64(&mut pop, frame)?;
                push(&mut *frame, WasmValue::I64(a as i16 as i64));
            }
            0xC4 => {
                let a = pop_i64(&mut pop, frame)?;
                push(&mut *frame, WasmValue::I64(a as i32 as i64));
            }
            other => return Err(format!("Unsupported numeric opcode 0x{other:02X}")),
        }
        Ok(())
    }
}

fn pop_i32(
    pop: &mut impl FnMut(&mut Frame) -> Result<WasmValue, String>,
    frame: &mut Frame,
) -> Result<i32, String> {
    match pop(frame)? {
        WasmValue::I32(value) => Ok(value),
        other => Err(format!("Expected i32, got {other:?}")),
    }
}

fn pop_u32(
    pop: &mut impl FnMut(&mut Frame) -> Result<WasmValue, String>,
    frame: &mut Frame,
) -> Result<u32, String> {
    pop_i32(pop, frame).map(|value| value as u32)
}

fn pop_i64(
    pop: &mut impl FnMut(&mut Frame) -> Result<WasmValue, String>,
    frame: &mut Frame,
) -> Result<i64, String> {
    match pop(frame)? {
        WasmValue::I64(value) => Ok(value),
        other => Err(format!("Expected i64, got {other:?}")),
    }
}

fn pop_u64(
    pop: &mut impl FnMut(&mut Frame) -> Result<WasmValue, String>,
    frame: &mut Frame,
) -> Result<u64, String> {
    pop_i64(pop, frame).map(|value| value as u64)
}

fn pop_f32(
    pop: &mut impl FnMut(&mut Frame) -> Result<WasmValue, String>,
    frame: &mut Frame,
) -> Result<f32, String> {
    match pop(frame)? {
        WasmValue::F32(value) => Ok(value),
        other => Err(format!("Expected f32, got {other:?}")),
    }
}

fn pop_f64(
    pop: &mut impl FnMut(&mut Frame) -> Result<WasmValue, String>,
    frame: &mut Frame,
) -> Result<f64, String> {
    match pop(frame)? {
        WasmValue::F64(value) => Ok(value),
        other => Err(format!("Expected f64, got {other:?}")),
    }
}

fn wasm_min_f32(a: f32, b: f32) -> f32 {
    if a.is_nan() || b.is_nan() {
        return f32::NAN;
    }
    if a == 0.0 && b == 0.0 {
        return if a.is_sign_negative() { a } else { b };
    }
    if a < b {
        a
    } else {
        b
    }
}

fn wasm_max_f32(a: f32, b: f32) -> f32 {
    if a.is_nan() || b.is_nan() {
        return f32::NAN;
    }
    if a == 0.0 && b == 0.0 {
        return if a.is_sign_positive() { a } else { b };
    }
    if a > b {
        a
    } else {
        b
    }
}

fn wasm_min_f64(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        return f64::NAN;
    }
    if a == 0.0 && b == 0.0 {
        return if a.is_sign_negative() { a } else { b };
    }
    if a < b {
        a
    } else {
        b
    }
}

fn wasm_max_f64(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        return f64::NAN;
    }
    if a == 0.0 && b == 0.0 {
        return if a.is_sign_positive() { a } else { b };
    }
    if a > b {
        a
    } else {
        b
    }
}

fn trunc_f64_to_i32(value: f64) -> Result<i32, String> {
    if value.is_nan() {
        return Err("invalid conversion to integer".to_string());
    }
    let truncated = value.trunc();
    if !(-2_147_483_648.0..2_147_483_648.0).contains(&truncated) {
        return Err("integer overflow".to_string());
    }
    Ok(truncated as i32)
}

fn trunc_f64_to_u32(value: f64) -> Result<u32, String> {
    if value.is_nan() {
        return Err("invalid conversion to integer".to_string());
    }
    let truncated = value.trunc();
    if !(truncated > -1.0 && truncated < 4_294_967_296.0) {
        return Err("integer overflow".to_string());
    }
    Ok(truncated as u32)
}

fn trunc_f64_to_i64(value: f64) -> Result<i64, String> {
    if value.is_nan() {
        return Err("invalid conversion to integer".to_string());
    }
    let truncated = value.trunc();
    if !(-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&truncated) {
        return Err("integer overflow".to_string());
    }
    Ok(truncated as i64)
}

fn trunc_f64_to_u64(value: f64) -> Result<u64, String> {
    if value.is_nan() {
        return Err("invalid conversion to integer".to_string());
    }
    let truncated = value.trunc();
    if !(truncated > -1.0 && truncated < 18_446_744_073_709_551_616.0) {
        return Err("integer overflow".to_string());
    }
    Ok(truncated as u64)
}

fn skip_immediates(code: &[u8], pc: usize) -> Result<usize, String> {
    let opcode = *code
        .get(pc)
        .ok_or_else(|| "Truncated instruction".to_string())?;
    let mut cursor = pc + 1;

    let leb = |cursor: &mut usize, max: usize| -> Result<(), String> {
        for _ in 0..max {
            let byte = *code
                .get(*cursor)
                .ok_or_else(|| "Truncated immediate".to_string())?;
            *cursor += 1;
            if byte & 0x80 == 0 {
                return Ok(());
            }
        }
        Err("Immediate too long".to_string())
    };
    match opcode {
        0x02..=0x04 => {
            let block_type = *code
                .get(cursor)
                .ok_or_else(|| "Missing block type".to_string())?;
            cursor += 1;
            if !matches!(block_type, 0x40 | 0x7F | 0x7E | 0x7D | 0x7C) {
                leb(&mut cursor, 5)?;
            }
        }
        0x0C | 0x0D | 0x10 | 0x12 | 0x13 => leb(&mut cursor, 5)?,
        0x11 => {
            leb(&mut cursor, 5)?;
            leb(&mut cursor, 5)?;
        }
        0x0E => {
            let mut count = 0u32;
            for shift in (0..20).step_by(7) {
                let byte = *code
                    .get(cursor)
                    .ok_or_else(|| "Truncated br_table".to_string())?;
                cursor += 1;
                count |= u32::from(byte & 0x7F) << shift;
                if byte & 0x80 == 0 {
                    break;
                }
            }
            for _ in 0..=count.min(MAX_LABELS as u32) {
                leb(&mut cursor, 5)?;
            }
        }
        0x1C => {
            let mut count = 0u32;
            for shift in (0..20).step_by(7) {
                let byte = *code
                    .get(cursor)
                    .ok_or_else(|| "Truncated select_t".to_string())?;
                cursor += 1;
                count |= u32::from(byte & 0x7F) << shift;
                if byte & 0x80 == 0 {
                    break;
                }
            }
            cursor = cursor.saturating_add(count.min(4) as usize);
        }
        0x20..=0x24 => leb(&mut cursor, 5)?,
        0x28..=0x3E => {
            leb(&mut cursor, 5)?;
            leb(&mut cursor, 5)?;
        }
        0x3F | 0x40 => cursor += 1,
        0x41 | 0x42 => leb(&mut cursor, 10)?,
        0x43 => cursor += 4,
        0x44 => cursor += 8,
        _ => {}
    }
    Ok(cursor)
}

fn find_end(code: &[u8], pc: usize) -> Result<usize, String> {
    let mut cursor = pc;
    let mut depth = 0usize;
    while cursor < code.len() {
        match code[cursor] {
            0x02..=0x04 => depth += 1,
            0x0B => {
                if depth == 0 {
                    return Ok(cursor);
                }
                depth -= 1;
            }
            _ => {}
        }
        cursor = skip_immediates(code, cursor)?;
    }
    Err("Unbalanced block: missing end".to_string())
}

fn find_else_or_end(code: &[u8], pc: usize) -> Result<usize, String> {
    let mut cursor = pc;
    let mut depth = 0usize;
    while cursor < code.len() {
        match code[cursor] {
            0x02..=0x04 => depth += 1,
            0x05 if depth == 0 => return Ok(cursor + 1),
            0x0B if depth == 0 => return Ok(cursor + 1),
            0x0B => depth -= 1,
            _ => {}
        }
        cursor = skip_immediates(code, cursor)?;
    }
    Err("Unbalanced block: missing end".to_string())
}

#[derive(Debug)]
enum Action {
    Continue,
    Branch(u32),
    Call(u32),
    CallIndirect(u32),
    Return,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wasm::parse_module;

    fn code_section(bodies: &[Vec<u8>]) -> Vec<u8> {
        let mut out = vec![0x0A];
        let mut content = vec![bodies.len() as u8];
        for body in bodies {
            content.push(body.len() as u8);
            content.extend_from_slice(body);
        }
        out.push(content.len() as u8);
        out.extend(content);
        out
    }

    fn body_no_locals(instructions: &[u8]) -> Vec<u8> {
        let mut body = vec![0x00];
        body.extend_from_slice(instructions);
        body.push(0x0B);
        body
    }

    fn add_module() -> WasmModule {
        let mut bytes = vec![0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00];

        bytes.extend([0x01, 0x07, 0x01, 0x60, 0x02, 0x7F, 0x7F, 0x01, 0x7F]);

        bytes.extend([0x03, 0x02, 0x01, 0x00]);

        bytes.extend([0x07, 0x07, 0x01, 0x03]);
        bytes.extend(b"add");
        bytes.extend([0x00, 0x00]);

        let body = body_no_locals(&[0x20, 0x00, 0x20, 0x01, 0x6A]);
        bytes.extend(code_section(&[body]));
        parse_module(&bytes).expect("module must parse")
    }

    fn fact_module() -> WasmModule {
        let mut bytes = vec![0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00];

        bytes.extend([0x01, 0x0C, 0x02]);
        bytes.extend([0x60, 0x01, 0x7F, 0x01, 0x7F]);
        bytes.extend([0x60, 0x02, 0x7F, 0x7F, 0x01, 0x7F]);

        bytes.extend([0x03, 0x02, 0x01, 0x00]);

        bytes.extend([0x07, 0x08, 0x01, 0x04]);
        bytes.extend(b"fact");
        bytes.extend([0x00, 0x00]);

        let body = body_no_locals(&[
            0x20, 0x00, 0x41, 0x02, 0x48, 0x04, 0x7F, 0x41, 0x01, 0x05, 0x20, 0x00, 0x20, 0x00,
            0x41, 0x01, 0x6B, 0x10, 0x00, 0x6C, 0x0B,
        ]);
        bytes.extend(code_section(&[body]));
        parse_module(&bytes).expect("module must parse")
    }

    #[test]
    fn add_function_returns_sum() {
        let module = add_module();
        let mut instance = WasmInstance::instantiate(module).unwrap();
        let results = instance
            .invoke(0, &[WasmValue::I32(20), WasmValue::I32(22)])
            .unwrap();
        assert_eq!(results, vec![WasmValue::I32(42)]);
    }

    #[test]
    fn recursive_factorial_matches_expected_values() {
        let module = fact_module();
        let mut instance = WasmInstance::instantiate(module).unwrap();
        for (input, expected) in [(0, 1), (1, 1), (5, 120), (10, 3_628_800)] {
            let results = instance.invoke(0, &[WasmValue::I32(input)]).unwrap();
            assert_eq!(results, vec![WasmValue::I32(expected)]);
        }
    }

    #[test]
    fn wrong_argument_count_fails_closed() {
        let module = add_module();
        let mut instance = WasmInstance::instantiate(module).unwrap();
        assert!(instance.invoke(0, &[WasmValue::I32(1)]).is_err());
    }

    #[test]
    fn missing_exported_function_name_is_none() {
        let module = add_module();
        let instance = WasmInstance::instantiate(module).unwrap();
        assert_eq!(instance.exported_function("nope"), None);
        assert_eq!(instance.exported_function("add"), Some(0));
    }

    #[test]
    fn deep_recursion_fails_closed_without_panicking() {
        let module = fact_module();
        let mut instance = WasmInstance::instantiate(module).unwrap();
        let error = instance.invoke(0, &[WasmValue::I32(2_000)]).unwrap_err();
        assert!(error.contains("depth"), "got: {error}");
    }

    #[test]
    fn memory_grow_is_bounded_and_loads_stay_in_range() {
        let mut full = vec![0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00];
        full.extend([0x01, 0x09, 0x02]);
        full.extend([0x60, 0x01, 0x7F, 0x00]);
        full.extend([0x60, 0x00, 0x01, 0x7F]);
        full.extend([0x03, 0x03, 0x02, 0x00, 0x01]);
        full.extend([0x05, 0x03, 0x01, 0x00, 0x01]);
        full.extend([0x07, 0x10, 0x02]);
        full.extend([0x05]);
        full.extend(b"store");
        full.extend([0x00, 0x00]);
        full.extend([0x04]);
        full.extend(b"load");
        full.extend([0x00, 0x01]);
        let store_body = body_no_locals(&[0x20, 0x00, 0x41, 0x2A, 0x36, 0x02, 0x00]);
        let load_body = body_no_locals(&[0x41, 0x08, 0x28, 0x02, 0x00]);
        full.extend(code_section(&[store_body, load_body]));
        let module = parse_module(&full).expect("module must parse");
        let mut instance = WasmInstance::instantiate(module).unwrap();
        assert_eq!(instance.memory_bytes(), 65_536);
        instance.invoke(0, &[WasmValue::I32(8)]).unwrap();
        let results = instance.invoke(1, &[]).unwrap();
        assert_eq!(results, vec![WasmValue::I32(42)]);

        let error = instance
            .invoke(0, &[WasmValue::I32(1_000_000)])
            .unwrap_err();
        assert!(error.contains("out of range"), "got: {error}");
    }
}

#[cfg(test)]
mod spec_regression_tests {
    use super::*;
    use crate::wasm::parse_module;

    fn code_section(bodies: &[Vec<u8>]) -> Vec<u8> {
        let mut section = vec![0x0A];
        let mut content = Vec::new();
        content.push(bodies.len() as u8);
        for body in bodies {
            content.push(body.len() as u8);
            content.extend_from_slice(body);
        }
        let count_len = 1;
        section.push((content.len()) as u8);
        section.extend(content);
        let _ = count_len;
        section
    }

    fn body_no_locals(instructions: &[u8]) -> Vec<u8> {
        let mut body = vec![0x00];
        body.extend_from_slice(instructions);
        body.push(0x0B);
        body
    }

    fn build_fn_module(name: &[u8], body_instructions: &[u8]) -> WasmModule {
        let mut bytes = vec![0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00];

        bytes.extend([0x01, 0x05, 0x01, 0x60, 0x00, 0x01, 0x7F]);
        bytes.extend([0x03, 0x02, 0x01, 0x00]);

        let export_size = 1 + 1 + name.len() + 2;
        bytes.push(0x07);
        bytes.push(export_size as u8);
        bytes.push(0x01);
        bytes.push(name.len() as u8);
        bytes.extend_from_slice(name);
        bytes.extend([0x00, 0x00]);
        let body = body_no_locals(body_instructions);
        bytes.extend(code_section(&[body]));
        parse_module(&bytes).expect("module must parse")
    }

    #[test]
    fn loop_fall_through_exits_instead_of_spinning() {
        let module = build_fn_module(b"loopexit", &[0x03, 0x40, 0x0B, 0x41, 0x2A]);
        let mut instance = WasmInstance::instantiate(module).unwrap();
        let results = instance.invoke(0, &[]).unwrap();
        assert_eq!(results, vec![WasmValue::I32(42)]);
    }

    #[test]
    fn branch_keeps_block_result_values() {
        let module = build_fn_module(b"brval", &[0x02, 0x7F, 0x41, 0x07, 0x0C, 0x00, 0x0B]);
        let mut instance = WasmInstance::instantiate(module).unwrap();
        let results = instance.invoke(0, &[]).unwrap();
        assert_eq!(results, vec![WasmValue::I32(7)]);
    }
}
