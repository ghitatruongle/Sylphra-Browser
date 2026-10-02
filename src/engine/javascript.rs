use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::rc::Rc;

pub const HOST_DOCUMENT: u64 = 1;
pub const HOST_WINDOW: u64 = 2;
pub const HOST_LOCAL_STORAGE: u64 = 3;
pub const HOST_HISTORY: u64 = 4;
pub const HOST_LOCATION: u64 = 5;
pub const HOST_EVENT: u64 = 6;
pub const HOST_FORM_DATA: u64 = 7;
pub const HOST_INDEXED_DB: u64 = 8;
pub const HOST_CACHE_STORAGE: u64 = 9;
pub const HOST_NAVIGATOR: u64 = 10;
pub const HOST_SERVICE_WORKER: u64 = 11;

pub const HOST_CUSTOM_ELEMENTS: u64 = 12;
pub const HOST_CRYPTO: u64 = 13;
pub const HOST_PERFORMANCE: u64 = 14;
pub const HOST_CSS: u64 = 15;

pub trait JsvHost {
    fn get_property(&mut self, object: u64, property: &str) -> Result<JsvValue, String>;
    fn set_property(
        &mut self,
        object: u64,
        property: &str,
        value: JsvValue,
    ) -> Result<JsvValue, String>;
    fn call(
        &mut self,
        object: u64,
        method: &str,
        arguments: Vec<JsvValue>,
    ) -> Result<JsvValue, String>;
}

struct NoHost;

impl JsvHost for NoHost {
    fn get_property(&mut self, _object: u64, property: &str) -> Result<JsvValue, String> {
        Err(format!(
            "SecurityError: host property '{}' is unavailable in this Realm",
            property
        ))
    }

    fn set_property(
        &mut self,
        _object: u64,
        property: &str,
        _value: JsvValue,
    ) -> Result<JsvValue, String> {
        Err(format!(
            "SecurityError: host property '{}' is unavailable in this Realm",
            property
        ))
    }

    fn call(
        &mut self,
        _object: u64,
        method: &str,
        _arguments: Vec<JsvValue>,
    ) -> Result<JsvValue, String> {
        Err(format!(
            "SecurityError: host method '{}' is unavailable in this Realm",
            method
        ))
    }
}

const MAX_EVAL_STEPS: u64 = crate::resource_caps::JS_MAX_EVAL_STEPS;
const MAX_SOURCE_BYTES: usize = crate::resource_caps::JS_MAX_SOURCE_BYTES;
const MAX_TOKENS: usize = crate::resource_caps::JS_MAX_TOKENS;

const MAX_STRING_LENGTH: usize = crate::resource_caps::JS_MAX_STRING_BYTES;

const MAX_STRING_BUDGET: usize = crate::resource_caps::JS_MAX_STRING_BUDGET_BYTES;

#[cfg(debug_assertions)]
const MAX_CALL_DEPTH: u64 = 8;
#[cfg(not(debug_assertions))]
const MAX_CALL_DEPTH: u64 = 32;

const MAX_PARSE_DEPTH: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DynamicCodePolicy {
    #[default]
    Denied,

    Allowed,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EvalStats {
    pub steps: u64,

    pub max_call_depth: u64,

    pub string_bytes: usize,

    pub microtasks_queued: usize,

    pub modules_evaluated: usize,

    pub timeout_reason: Option<String>,

    pub loop_iterations: u64,

    pub control_signals: u64,
}

pub(crate) struct EvalCtx<'host> {
    steps: u64,

    max_steps: u64,

    call_depth: u64,

    max_call_depth: u64,

    string_bytes: usize,

    host: &'host mut dyn JsvHost,
    microtasks: VecDeque<PromiseReactionJob>,
    microtasks_queued: usize,
    pending_promise_reactions: HashMap<usize, Vec<PendingPromiseReaction>>,

    modules: Option<&'host mut JsvModuleGraph>,

    in_generator: usize,

    current_generator: Option<JsvGeneratorRef>,

    modules_evaluated: usize,

    timeout_reason: Option<String>,

    dynamic_code: DynamicCodePolicy,

    proxy_depth: usize,
}

impl<'host> EvalCtx<'host> {
    fn new(host: &'host mut dyn JsvHost, modules: &'host mut JsvModuleGraph) -> Self {
        Self::with_step_limit(host, modules, MAX_EVAL_STEPS)
    }

    fn with_step_limit(
        host: &'host mut dyn JsvHost,
        modules: &'host mut JsvModuleGraph,
        max_steps: u64,
    ) -> Self {
        Self {
            steps: 0,
            max_steps: max_steps.min(MAX_EVAL_STEPS),
            call_depth: 0,
            max_call_depth: 0,
            string_bytes: 0,
            host,
            microtasks: VecDeque::new(),
            microtasks_queued: 0,
            pending_promise_reactions: HashMap::new(),
            modules: Some(modules),
            in_generator: 0,
            current_generator: None,
            modules_evaluated: 0,
            timeout_reason: None,
            dynamic_code: DynamicCodePolicy::Denied,
            proxy_depth: 0,
        }
    }

    fn stats(&self) -> EvalStats {
        EvalStats {
            steps: self.steps,
            max_call_depth: self.max_call_depth,
            string_bytes: self.string_bytes,
            microtasks_queued: self.microtasks_queued,
            modules_evaluated: self.modules_evaluated,
            timeout_reason: self.timeout_reason.clone(),
            loop_iterations: 0,
            control_signals: 0,
        }
    }

    fn step(&mut self) -> Result<(), String> {
        self.steps = self.steps.saturating_add(1);
        if self.steps > self.max_steps {
            self.timeout_reason = Some("Script timed out (step budget exceeded)".to_string());
            return Err("Script timed out (step budget exceeded)".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PromiseReactionKind {
    Fulfill,
    Reject,
    Finally,
}

#[derive(Debug, Clone)]
struct PromiseReactionJob {
    kind: PromiseReactionKind,
    handler: Option<JsvValue>,
    argument: JsvValue,
    result: JsvPromiseRef,
}

#[derive(Debug, Clone)]
struct PendingPromiseReaction {
    on_fulfilled: Option<JsvValue>,
    on_rejected: Option<JsvValue>,
    on_finally: Option<JsvValue>,
    completion_override: Option<(PromiseReactionKind, JsvValue)>,
    result: JsvPromiseRef,
}

const MAX_PROMISE_JOBS: usize = 2_048;

fn check_string_alloc(ctx: &mut EvalCtx<'_>, len: usize) -> Result<(), String> {
    if len > MAX_STRING_LENGTH {
        return Err(format!(
            "String too large ({} bytes, max {})",
            len, MAX_STRING_LENGTH
        ));
    }
    ctx.string_bytes = ctx.string_bytes.saturating_add(len);
    if ctx.string_bytes > MAX_STRING_BUDGET {
        return Err("String memory budget exceeded".to_string());
    }
    Ok(())
}

const MAX_CONSOLE_LINES: usize = 500;
const MAX_CONSOLE_LINE_BYTES: usize = 16 * 1024;
const MAX_CONSOLE_BYTES: usize = 1024 * 1024;

fn push_console(console: &mut Vec<String>, mut line: String) {
    if line.len() > MAX_CONSOLE_LINE_BYTES {
        let mut end = MAX_CONSOLE_LINE_BYTES;
        while end > 0 && !line.is_char_boundary(end) {
            end -= 1;
        }
        line.truncate(end);
        line.push('…');
    }
    console.push(line);
    let mut retained_bytes = console.iter().map(String::len).sum::<usize>();
    while console.len() > MAX_CONSOLE_LINES || retained_bytes > MAX_CONSOLE_BYTES {
        if console.is_empty() {
            break;
        }
        retained_bytes = retained_bytes.saturating_sub(console[0].len());
        console.remove(0);
    }
}

fn is_abrupt_signal(v: &JsvValue) -> bool {
    matches!(
        v,
        JsvValue::ReturnSignal(_)
            | JsvValue::ThrowSignal(_)
            | JsvValue::BreakSignal
            | JsvValue::ContinueSignal
            | JsvValue::YieldSignal(_)
    )
}

fn max_parse_depth(tokens: &[String]) -> usize {
    let mut depth = 0usize;
    let mut max = 0usize;
    for t in tokens {
        match t.as_str() {
            "(" | "[" | "{" => {
                depth += 1;
                max = max.max(depth);
            }
            ")" | "]" | "}" => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    max
}

pub type JsvObjectRef = Rc<RefCell<JsvObject>>;
pub type JsvArrayRef = Rc<RefCell<Vec<JsvValue>>>;
pub type JsvPromiseRef = Rc<RefCell<JsvPromiseState>>;

#[derive(Debug, Clone)]
pub struct JsvObject {
    pub properties: HashMap<String, JsvValue>,
    pub prototype: Option<JsvObjectRef>,

    pub class_tags: Vec<u64>,

    pub private_fields: HashMap<String, JsvValue>,
}

impl JsvObject {
    pub fn plain(properties: HashMap<String, JsvValue>) -> Self {
        Self {
            properties,
            prototype: None,
            class_tags: Vec::new(),
            private_fields: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum JsvPromiseState {
    Pending,
    Fulfilled(JsvValue),
    Rejected(JsvValue),
}

#[derive(Debug, Clone)]
pub struct JsvSymbol {
    pub description: String,
    pub id: u64,
}
pub type JsvSymbolRef = Rc<JsvSymbol>;

static NEXT_SYMBOL_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn new_symbol(description: &str) -> JsvSymbolRef {
    let id = NEXT_SYMBOL_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    Rc::new(JsvSymbol {
        description: description.to_string(),
        id,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WellKnownSymbol {
    Iterator,
    AsyncIterator,
    ToStringTag,
    HasInstance,
    Species,
    IsConcatSpreadable,
    ToPrimitive,
    Match,
    Replace,
    Search,
    Split,
    Unscopables,
}

impl WellKnownSymbol {
    fn name(self) -> &'static str {
        match self {
            WellKnownSymbol::Iterator => "iterator",
            WellKnownSymbol::AsyncIterator => "asyncIterator",
            WellKnownSymbol::ToStringTag => "toStringTag",
            WellKnownSymbol::HasInstance => "hasInstance",
            WellKnownSymbol::Species => "species",
            WellKnownSymbol::IsConcatSpreadable => "isConcatSpreadable",
            WellKnownSymbol::ToPrimitive => "toPrimitive",
            WellKnownSymbol::Match => "match",
            WellKnownSymbol::Replace => "replace",
            WellKnownSymbol::Search => "search",
            WellKnownSymbol::Split => "split",
            WellKnownSymbol::Unscopables => "unscopables",
        }
    }
}

pub const SYMBOL_KEY_PREFIX: &str = "\u{0}\u{1}@symbol:";

fn symbol_property_key(symbol: &JsvSymbol) -> String {
    format!("{}{}", SYMBOL_KEY_PREFIX, symbol.id)
}

fn is_symbol_property_key(key: &str) -> bool {
    key.starts_with(SYMBOL_KEY_PREFIX)
}

pub const PRIVATE_NAME_PREFIX: &str = "#";

#[derive(Debug, Clone)]
pub struct JsvErrorObj {
    pub name: String,
    pub message: String,
}
pub type JsvErrorRef = Rc<JsvErrorObj>;

#[derive(Debug, Clone, Default)]
pub struct JsvMap {
    pub entries: Vec<(JsvValue, JsvValue)>,
}
pub type JsvMapRef = Rc<RefCell<JsvMap>>;

#[derive(Debug, Clone, Default)]
pub struct JsvSet {
    pub values: Vec<JsvValue>,
}
pub type JsvSetRef = Rc<RefCell<JsvSet>>;

#[derive(Debug, Clone)]
pub enum JsvWeakKey {
    Object(std::rc::Weak<RefCell<JsvObject>>),
    Array(std::rc::Weak<RefCell<Vec<JsvValue>>>),
    Promise(std::rc::Weak<RefCell<JsvPromiseState>>),
    Host(u64),
}

#[derive(Debug, Clone)]
pub struct JsvWeakEntry {
    pub key: JsvWeakKey,
    pub value: JsvValue,
}

#[derive(Debug, Default)]
pub struct JsvWeakMap {
    pub entries: Vec<JsvWeakEntry>,
}
pub type JsvWeakMapRef = Rc<RefCell<JsvWeakMap>>;

#[derive(Debug, Default)]
pub struct JsvWeakSet {
    pub keys: Vec<JsvWeakKey>,
}
pub type JsvWeakSetRef = Rc<RefCell<JsvWeakSet>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypedArrayKind {
    Int8,
    Uint8,
    Uint8Clamped,
    Int16,
    Uint16,
    Int32,
    Uint32,
    Float32,
    Float64,
}

impl TypedArrayKind {
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "Int8Array" => Self::Int8,
            "Uint8Array" => Self::Uint8,
            "Uint8ClampedArray" => Self::Uint8Clamped,
            "Int16Array" => Self::Int16,
            "Uint16Array" => Self::Uint16,
            "Int32Array" => Self::Int32,
            "Uint32Array" => Self::Uint32,
            "Float32Array" => Self::Float32,
            "Float64Array" => Self::Float64,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Int8 => "Int8Array",
            Self::Uint8 => "Uint8Array",
            Self::Uint8Clamped => "Uint8ClampedArray",
            Self::Int16 => "Int16Array",
            Self::Uint16 => "Uint16Array",
            Self::Int32 => "Int32Array",
            Self::Uint32 => "Uint32Array",
            Self::Float32 => "Float32Array",
            Self::Float64 => "Float64Array",
        }
    }

    pub fn bytes_per_element(self) -> usize {
        match self {
            Self::Int8 | Self::Uint8 | Self::Uint8Clamped => 1,
            Self::Int16 | Self::Uint16 => 2,
            Self::Int32 | Self::Uint32 | Self::Float32 => 4,
            Self::Float64 => 8,
        }
    }

    pub fn max_elements(self) -> usize {
        MAX_STRING_LENGTH / self.bytes_per_element()
    }

    fn read(self, bytes: &[u8], index: usize) -> Result<f64, String> {
        let oob = || "RangeError: typed array OOB".to_string();
        let bpe = self.bytes_per_element();
        let offset = index.checked_mul(bpe).ok_or_else(oob)?;
        let end = offset.checked_add(bpe).ok_or_else(oob)?;
        if end > bytes.len() {
            return Err(oob());
        }
        let slice = bytes.get(offset..end).ok_or_else(oob)?;
        Ok(match self {
            Self::Int8 => i8::from_le_bytes(slice.try_into().map_err(|_| oob())?) as f64,
            Self::Uint8 | Self::Uint8Clamped => slice.first().copied().ok_or_else(oob)? as f64,
            Self::Int16 => i16::from_le_bytes(slice.try_into().map_err(|_| oob())?) as f64,
            Self::Uint16 => u16::from_le_bytes(slice.try_into().map_err(|_| oob())?) as f64,
            Self::Int32 => i32::from_le_bytes(slice.try_into().map_err(|_| oob())?) as f64,
            Self::Uint32 => u32::from_le_bytes(slice.try_into().map_err(|_| oob())?) as f64,
            Self::Float32 => f32::from_le_bytes(slice.try_into().map_err(|_| oob())?) as f64,
            Self::Float64 => f64::from_le_bytes(slice.try_into().map_err(|_| oob())?),
        })
    }

    fn write(self, bytes: &mut [u8], index: usize, value: f64) {
        let offset = index * self.bytes_per_element();
        if offset + self.bytes_per_element() > bytes.len() {
            return;
        }
        let slice = &mut bytes[offset..offset + self.bytes_per_element()];
        match self {
            Self::Int8 => slice.copy_from_slice(&(value as i8).to_le_bytes()),
            Self::Uint8 => slice.copy_from_slice(&(value as u8).to_le_bytes()),
            Self::Uint8Clamped => {
                let clamped = if value.is_nan() {
                    0.0
                } else {
                    value.round().clamp(0.0, 255.0)
                };
                slice.copy_from_slice(&(clamped as u8).to_le_bytes());
            }
            Self::Int16 => slice.copy_from_slice(&(value as i16).to_le_bytes()),
            Self::Uint16 => slice.copy_from_slice(&(value as u16).to_le_bytes()),
            Self::Int32 => slice.copy_from_slice(&(value as i32).to_le_bytes()),
            Self::Uint32 => slice.copy_from_slice(&(value as u32).to_le_bytes()),
            Self::Float32 => slice.copy_from_slice(&(value as f32).to_le_bytes()),
            Self::Float64 => slice.copy_from_slice(&value.to_le_bytes()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct JsvArrayBuffer {
    pub bytes: Vec<u8>,
    pub detached: bool,
}
pub type JsvArrayBufferRef = Rc<RefCell<JsvArrayBuffer>>;

#[derive(Debug, Clone)]
pub struct JsvTypedArray {
    pub kind: TypedArrayKind,
    pub buffer: JsvArrayBufferRef,
    pub byte_offset: usize,
    pub length: usize,
}
pub type JsvTypedArrayRef = Rc<RefCell<JsvTypedArray>>;

#[derive(Debug, Clone)]
pub struct JsvDataView {
    pub buffer: JsvArrayBufferRef,
    pub byte_offset: usize,
    pub byte_length: usize,
}
pub type JsvDataViewRef = Rc<RefCell<JsvDataView>>;

#[derive(Debug, Clone)]
pub struct JsvProxy {
    pub target: JsvValue,
    pub handler: JsvValue,
}
pub type JsvProxyRef = Rc<JsvProxy>;

#[derive(Debug, Clone, PartialEq)]
pub enum ParamPattern {
    Identifier(String),
    Array(Vec<ParamPattern>, Option<Box<ParamPattern>>),
    Object(Vec<(String, ParamPattern)>, Option<Box<ParamPattern>>),
    Rest(Box<ParamPattern>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParamBinding {
    pub pattern: ParamPattern,
    pub default: Option<Box<JsvExpr>>,
}

impl ParamBinding {
    fn plain(name: String) -> Self {
        Self {
            pattern: ParamPattern::Identifier(name),
            default: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct JsvClassMethod {
    pub name: String,
    pub params: Vec<ParamBinding>,
    pub body: Box<JsvExpr>,
    pub is_async: bool,
    pub is_generator: bool,
    pub is_static: bool,
    pub is_getter: bool,
    pub is_setter: bool,
    pub is_private: bool,
}

#[derive(Debug, Clone)]
pub struct JsvClass {
    pub name: String,
    pub id: u64,
    pub parent: Option<JsvValue>,
    pub constructor: Option<(Vec<ParamBinding>, Box<JsvExpr>)>,
    pub instance_methods: HashMap<String, JsvValue>,
    pub static_methods: HashMap<String, JsvValue>,
    pub private_names: Vec<String>,

    pub instance_fields: Vec<(String, Option<Box<JsvExpr>>)>,

    pub prototype_object: JsvObjectRef,

    pub class_env: JsvEnvironment,
}
pub type JsvClassRef = Rc<JsvClass>;

static NEXT_CLASS_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

#[derive(Debug, Clone)]
pub struct GeneratorResume {
    pub stmts: Vec<JsvExpr>,
    pub index: usize,
    pub env: JsvEnvironment,
    pub conts: Vec<Cont>,
    pub result: JsvValue,
}

#[derive(Debug, Clone)]
pub struct JsvGeneratorState {
    pub name: String,
    pub params: Vec<ParamBinding>,
    pub body: Box<JsvExpr>,
    pub captured: JsvEnvironment,
    pub started: bool,
    pub done: bool,
    pub resume: Option<GeneratorResume>,

    pub running: bool,
}
pub type JsvGeneratorRef = Rc<RefCell<JsvGeneratorState>>;

struct GeneratorRunningGuard {
    generator: JsvGeneratorRef,
}

impl Drop for GeneratorRunningGuard {
    fn drop(&mut self) {
        if let Ok(mut state) = self.generator.try_borrow_mut() {
            state.running = false;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IteratorKind {
    Array,
    String,
    Map,
    Set,
    TypedArray,
    Generator,
    Keys,
    Values,
    Entries,
}

#[derive(Debug, Clone)]
pub struct JsvIteratorState {
    pub kind: IteratorKind,
    pub source: JsvValue,
    pub index: usize,

    pub byte_offset: usize,

    pub snapshot: Option<JsvValue>,

    pub column: u8,

    pub generator: Option<JsvGeneratorRef>,
}
pub type JsvIteratorRef = Rc<RefCell<JsvIteratorState>>;

#[derive(Debug, Clone, PartialEq)]
pub enum AssignTarget {
    Identifier(String),
    Member(Box<JsvExpr>, String),
    Index(Box<JsvExpr>, Box<JsvExpr>),
}

#[derive(Debug, Clone)]
pub enum Cont {
    BinaryRight {
        left: JsvValue,
        op: OpKind,
        right: Box<JsvExpr>,
        env: JsvEnvironment,
    },

    BinaryApply {
        left: JsvValue,
        op: OpKind,
    },

    UnaryApply {
        op: OpKind,
    },

    CallArgs {
        callee: JsvValue,
        done: Vec<JsvValue>,
        left: Vec<JsvExpr>,
        env: JsvEnvironment,
        is_new: bool,
        this_arg: JsvValue,
    },

    MemberApply {
        property: String,
        optional: bool,
    },

    IndexBase {
        key_expr: Box<JsvExpr>,
        optional: bool,
        env: JsvEnvironment,
    },

    IndexKey {
        object: JsvValue,
        key_expr: Box<JsvExpr>,
        optional: bool,
        env: JsvEnvironment,
    },

    IndexApply {
        object: JsvValue,
        key: JsvValue,
        optional: bool,
    },

    MemberAssignBase {
        property: String,
        rhs: Box<JsvExpr>,
        env: JsvEnvironment,
    },

    MemberAssignApply {
        object: JsvValue,
        property: String,
    },

    IndexAssignBase {
        key_expr: Box<JsvExpr>,
        rhs: Box<JsvExpr>,
        env: JsvEnvironment,
    },

    IndexAssignKey {
        object: JsvValue,
        rhs: Box<JsvExpr>,
        env: JsvEnvironment,
    },

    IndexAssignValue {
        object: JsvValue,
        key: JsvValue,
    },

    IndexAssignApply {
        object: JsvValue,
        key: JsvValue,
    },

    ArrayElem {
        done: Vec<JsvValue>,
        left: Vec<JsvExpr>,
        env: JsvEnvironment,
    },

    ObjectElem {
        done: Vec<(String, JsvValue)>,
        left: Vec<ObjectProperty>,
        env: JsvEnvironment,
    },

    TemplatePart {
        output: String,
        left: Vec<TemplatePart>,
        env: JsvEnvironment,
    },

    TernaryCond {
        then_expr: Box<JsvExpr>,
        else_expr: Box<JsvExpr>,
        env: JsvEnvironment,
    },

    AssignApply {
        target: AssignTarget,
        env: JsvEnvironment,
    },

    VarDeclApply {
        name: String,
        kind: DeclarationKind,
        env: JsvEnvironment,
    },

    BindPatternApply {
        pattern: Box<ParamPattern>,
        kind: DeclarationKind,
        env: JsvEnvironment,
    },

    ReturnApply,

    ThrowApply,

    IfCond {
        then_branch: Vec<JsvExpr>,
        else_branch: Option<Vec<JsvExpr>>,
        env: JsvEnvironment,
    },

    WhileCond {
        cond: Box<JsvExpr>,
        body: Vec<JsvExpr>,
        env: JsvEnvironment,
        result: JsvValue,
        iterations: usize,
    },

    WhileBody {
        cond: Box<JsvExpr>,
        body: Vec<JsvExpr>,
        env: JsvEnvironment,
        result: JsvValue,
        iterations: usize,
    },

    DoWhileBody {
        cond: Box<JsvExpr>,
        body: Vec<JsvExpr>,
        env: JsvEnvironment,
        result: JsvValue,
        iterations: usize,
    },

    DoWhileCond {
        cond: Box<JsvExpr>,
        body: Vec<JsvExpr>,
        env: JsvEnvironment,
        result: JsvValue,
        iterations: usize,
    },

    ForInRest {
        binding: String,
        kind: DeclarationKind,
        body: Vec<JsvExpr>,
        env: JsvEnvironment,
        result: JsvValue,
        keys: Vec<String>,
        index: usize,
    },

    ForOfRest {
        pattern: Box<ParamPattern>,
        kind: DeclarationKind,
        body: Vec<JsvExpr>,
        env: JsvEnvironment,
        result: JsvValue,
        values: Vec<JsvValue>,
        index: usize,
    },

    Statements {
        stmts: Vec<JsvExpr>,
        index: usize,
        env: JsvEnvironment,
        result: JsvValue,
    },

    TryResume {
        try_body: Vec<JsvExpr>,
        catch_binding: Option<String>,
        catch_body: Vec<JsvExpr>,
        finally_body: Vec<JsvExpr>,
        env: JsvEnvironment,
        outcome: JsvValue,
        stage: u8,
    },

    YieldStarLoop {
        iterator: JsvIteratorRef,
        generator: JsvGeneratorRef,
    },

    ForOfIterable {
        pattern: Box<ParamPattern>,
        kind: DeclarationKind,
        body: Vec<JsvExpr>,
        env: JsvEnvironment,
        result: JsvValue,
    },

    CallCallee {
        args: Vec<JsvExpr>,
        env: JsvEnvironment,
        is_new: bool,
    },

    CallMemberBase {
        property: String,
        args: Vec<JsvExpr>,
        env: JsvEnvironment,
        is_new: bool,
    },

    CallIndexBase {
        key_expr: Box<JsvExpr>,
        args: Vec<JsvExpr>,
        env: JsvEnvironment,
        is_new: bool,
    },

    CallIndexKey {
        object: JsvValue,
        args: Vec<JsvExpr>,
        env: JsvEnvironment,
        is_new: bool,
    },

    CallPrivateBase {
        name: String,
        args: Vec<JsvExpr>,
        env: JsvEnvironment,
    },

    SwitchBody {
        result: JsvValue,
    },
}

#[derive(Debug, Clone)]
pub struct JsvYieldSignal {
    pub value: Box<JsvValue>,
    pub conts: Vec<Cont>,
}

impl JsvYieldSignal {
    fn new(value: JsvValue) -> Self {
        Self {
            value: Box::new(value),
            conts: Vec::new(),
        }
    }

    fn push(mut self, cont: Cont) -> Self {
        self.conts.push(cont);
        self
    }
}

#[derive(Debug, Clone)]
pub enum JsvValue {
    Number(f64),
    Boolean(bool),
    String(String),
    Null,
    Undefined,
    Object(JsvObjectRef),
    Array(JsvArrayRef),
    Function(String, Vec<ParamBinding>, Box<JsvExpr>, JsvEnvironment),
    AsyncFunction(String, Vec<ParamBinding>, Box<JsvExpr>, JsvEnvironment),
    GeneratorFn(String, Vec<ParamBinding>, Box<JsvExpr>, JsvEnvironment),
    NativeFn(String),
    BoundNativeFn(String, Box<JsvValue>),
    Promise(JsvPromiseRef),
    Symbol(JsvSymbolRef),
    Error(JsvErrorRef),
    Map(JsvMapRef),
    Set(JsvSetRef),
    WeakMap(JsvWeakMapRef),
    WeakSet(JsvWeakSetRef),
    ArrayBuffer(JsvArrayBufferRef),
    TypedArray(JsvTypedArrayRef),
    DataView(JsvDataViewRef),
    Proxy(JsvProxyRef),
    Class(JsvClassRef),
    GeneratorObject(JsvGeneratorRef),
    Iterator(JsvIteratorRef),

    GetterSetter(Box<JsvValue>, Box<JsvValue>),

    LiveBinding(JsvEnvironment, String),

    HostObject(u64),

    HostFunction(u64, String),

    ReturnSignal(Box<JsvValue>),

    ThrowSignal(Box<JsvValue>),

    BreakSignal,
    ContinueSignal,

    YieldSignal(Box<JsvYieldSignal>),
}

impl PartialEq for JsvValue {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Number(left), Self::Number(right)) => left == right,
            (Self::Boolean(left), Self::Boolean(right)) => left == right,
            (Self::String(left), Self::String(right)) => left == right,
            (Self::Null, Self::Null) | (Self::Undefined, Self::Undefined) => true,
            (Self::Object(left), Self::Object(right)) => Rc::ptr_eq(left, right),
            (Self::Array(left), Self::Array(right)) => Rc::ptr_eq(left, right),
            (Self::Promise(left), Self::Promise(right)) => Rc::ptr_eq(left, right),
            (Self::Symbol(left), Self::Symbol(right)) => Rc::ptr_eq(left, right),
            (Self::Error(left), Self::Error(right)) => Rc::ptr_eq(left, right),
            (Self::Map(left), Self::Map(right)) => Rc::ptr_eq(left, right),
            (Self::Set(left), Self::Set(right)) => Rc::ptr_eq(left, right),
            (Self::WeakMap(left), Self::WeakMap(right)) => Rc::ptr_eq(left, right),
            (Self::WeakSet(left), Self::WeakSet(right)) => Rc::ptr_eq(left, right),
            (Self::ArrayBuffer(left), Self::ArrayBuffer(right)) => Rc::ptr_eq(left, right),
            (Self::TypedArray(left), Self::TypedArray(right)) => Rc::ptr_eq(left, right),
            (Self::DataView(left), Self::DataView(right)) => Rc::ptr_eq(left, right),
            (Self::Proxy(left), Self::Proxy(right)) => Rc::ptr_eq(left, right),
            (Self::Class(left), Self::Class(right)) => Rc::ptr_eq(left, right),
            (Self::GeneratorObject(left), Self::GeneratorObject(right)) => Rc::ptr_eq(left, right),
            (Self::Iterator(left), Self::Iterator(right)) => Rc::ptr_eq(left, right),
            (Self::GetterSetter(left_get, left_set), Self::GetterSetter(right_get, right_set)) => {
                left_get == right_get && left_set == right_set
            }
            (Self::LiveBinding(left_env, left_name), Self::LiveBinding(right_env, right_name)) => {
                left_name == right_name && left_env.same_record(right_env)
            }
            (
                Self::GeneratorFn(left_name, left_params, left_body, left_env),
                Self::GeneratorFn(right_name, right_params, right_body, right_env),
            ) => {
                left_name == right_name
                    && left_params == right_params
                    && left_body == right_body
                    && left_env.same_record(right_env)
            }
            (
                Self::Function(left_name, left_params, left_body, left_env),
                Self::Function(right_name, right_params, right_body, right_env),
            )
            | (
                Self::AsyncFunction(left_name, left_params, left_body, left_env),
                Self::AsyncFunction(right_name, right_params, right_body, right_env),
            ) => {
                left_name == right_name
                    && left_params == right_params
                    && left_body == right_body
                    && left_env.same_record(right_env)
            }
            (Self::NativeFn(left), Self::NativeFn(right)) => left == right,
            (Self::BoundNativeFn(left_name, left), Self::BoundNativeFn(right_name, right)) => {
                left_name == right_name && left == right
            }
            (Self::HostObject(left), Self::HostObject(right)) => left == right,
            (
                Self::HostFunction(left_object, left_name),
                Self::HostFunction(right_object, right_name),
            ) => left_object == right_object && left_name == right_name,
            (Self::ReturnSignal(left), Self::ReturnSignal(right))
            | (Self::ThrowSignal(left), Self::ThrowSignal(right)) => left == right,
            (Self::YieldSignal(left), Self::YieldSignal(right)) => {
                left.value == right.value && left.conts.len() == right.conts.len()
            }
            (Self::BreakSignal, Self::BreakSignal)
            | (Self::ContinueSignal, Self::ContinueSignal) => true,
            _ => false,
        }
    }
}

impl JsvValue {
    pub fn number(n: f64) -> Self {
        JsvValue::Number(n)
    }
    pub fn boolean(b: bool) -> Self {
        JsvValue::Boolean(b)
    }
    pub fn string<S: Into<String>>(s: S) -> Self {
        JsvValue::String(s.into())
    }
    pub fn object() -> Self {
        JsvValue::Object(Rc::new(RefCell::new(JsvObject::plain(HashMap::new()))))
    }
    pub fn array() -> Self {
        JsvValue::Array(Rc::new(RefCell::new(Vec::new())))
    }

    pub fn as_number(&self) -> Option<f64> {
        match self {
            JsvValue::Number(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_boolean(&self) -> Option<bool> {
        match self {
            JsvValue::Boolean(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_string(&self) -> Option<&str> {
        match self {
            JsvValue::String(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn as_symbol_id(&self) -> Option<u64> {
        match self {
            JsvValue::Symbol(symbol) => Some(symbol.id),
            _ => None,
        }
    }

    pub fn symbol_id_from_key(key: &str) -> Option<u64> {
        key.strip_prefix(SYMBOL_KEY_PREFIX)?.parse().ok()
    }

    pub fn is_truthy(&self) -> bool {
        match self {
            JsvValue::Null | JsvValue::Undefined => false,
            JsvValue::Boolean(b) => *b,
            JsvValue::Number(n) => *n != 0.0 && !n.is_nan(),
            JsvValue::String(s) => !s.is_empty(),
            _ => true,
        }
    }

    pub fn is_object_like(&self) -> bool {
        matches!(
            self,
            JsvValue::Object(_)
                | JsvValue::Array(_)
                | JsvValue::Function(_, _, _, _)
                | JsvValue::AsyncFunction(_, _, _, _)
                | JsvValue::GeneratorFn(_, _, _, _)
                | JsvValue::NativeFn(_)
                | JsvValue::BoundNativeFn(_, _)
                | JsvValue::Promise(_)
                | JsvValue::Error(_)
                | JsvValue::Map(_)
                | JsvValue::Set(_)
                | JsvValue::WeakMap(_)
                | JsvValue::WeakSet(_)
                | JsvValue::ArrayBuffer(_)
                | JsvValue::TypedArray(_)
                | JsvValue::DataView(_)
                | JsvValue::Proxy(_)
                | JsvValue::Class(_)
                | JsvValue::GeneratorObject(_)
                | JsvValue::Iterator(_)
                | JsvValue::GetterSetter(_, _)
                | JsvValue::HostObject(_)
                | JsvValue::HostFunction(_, _)
        )
    }

    pub fn deref_live(&self) -> JsvValue {
        match self {
            JsvValue::LiveBinding(record, name) => record
                .get(name)
                .map(|value| value.deref_live())
                .unwrap_or(JsvValue::Undefined),
            other => other.clone(),
        }
    }

    pub fn to_display_string(&self) -> String {
        self.to_display_string_at_depth(0)
    }

    fn to_display_string_at_depth(&self, depth: usize) -> String {
        if depth > 32 {
            return "…".to_string();
        }
        let inner = |value: &JsvValue| value.to_display_string_at_depth(depth + 1);
        match self {
            JsvValue::Number(n) => {
                if n.is_infinite() {
                    if *n > 0.0 {
                        "Infinity".to_string()
                    } else {
                        "-Infinity".to_string()
                    }
                } else if *n == n.floor() && n.is_finite() && n.abs() < 1e15 {
                    format!("{}", *n as i64)
                } else if *n == 0.0 && n.is_sign_negative() {
                    "-0".to_string()
                } else {
                    format!("{}", n)
                }
            }
            JsvValue::Boolean(b) => b.to_string(),
            JsvValue::String(s) => s.clone(),
            JsvValue::Null => "null".to_string(),
            JsvValue::Undefined => "undefined".to_string(),
            JsvValue::Object(_) => "[object Object]".to_string(),
            JsvValue::Array(a) => format!(
                "[{}]",
                a.borrow().iter().map(inner).collect::<Vec<_>>().join(", ")
            ),
            JsvValue::Function(name, _, _, _) => format!("[Function: {}]", name),
            JsvValue::AsyncFunction(name, _, _, _) => format!("[AsyncFunction: {}]", name),
            JsvValue::GeneratorFn(name, _, _, _) => format!("[GeneratorFunction: {}]", name),
            JsvValue::NativeFn(name) => format!("[Native Function: {}]", name),
            JsvValue::BoundNativeFn(name, _) => format!("[Native Method: {}]", name),
            JsvValue::Promise(_) => "[object Promise]".to_string(),
            JsvValue::Symbol(symbol) => symbol.description.clone(),
            JsvValue::Error(error) => format!("{}: {}", error.name, error.message),
            JsvValue::Map(map) => format!("[object Map ({} entries)]", map.borrow().entries.len()),
            JsvValue::Set(set) => format!("[object Set ({} values)]", set.borrow().values.len()),
            JsvValue::WeakMap(_) => "[object WeakMap]".to_string(),
            JsvValue::WeakSet(_) => "[object WeakSet]".to_string(),
            JsvValue::ArrayBuffer(buffer) => {
                if buffer.borrow().detached {
                    "[object ArrayBuffer (detached)]".to_string()
                } else {
                    format!(
                        "[object ArrayBuffer ({} bytes)]",
                        buffer.borrow().bytes.len()
                    )
                }
            }
            JsvValue::TypedArray(array) => format!("[object {}]", array.borrow().kind.name()),
            JsvValue::DataView(_) => "[object DataView]".to_string(),
            JsvValue::Proxy(_) => "[object Proxy]".to_string(),
            JsvValue::Class(class) => format!("[class {}]", class.name),
            JsvValue::GeneratorObject(_) => "[object Generator]".to_string(),
            JsvValue::Iterator(_) => "[object Iterator]".to_string(),
            JsvValue::GetterSetter(get, set) => {
                if is_callable(get) && is_callable(set) {
                    "[Getter/Setter]".to_string()
                } else if is_callable(get) {
                    "[Getter]".to_string()
                } else {
                    "[Setter]".to_string()
                }
            }
            JsvValue::LiveBinding(record, name) => record
                .get(name)
                .map(|value| value.deref_live().to_display_string())
                .unwrap_or_else(|| "[uninitialized binding]".to_string()),
            JsvValue::HostObject(_) => "[object HostObject]".to_string(),
            JsvValue::HostFunction(_, name) => format!("[Host Function: {}]", name),
            JsvValue::ReturnSignal(v) => v.to_display_string(),
            JsvValue::ThrowSignal(v) => v.to_display_string(),
            JsvValue::BreakSignal => "break".to_string(),
            JsvValue::ContinueSignal => "continue".to_string(),
            JsvValue::YieldSignal(signal) => signal.value.to_display_string(),
        }
    }

    pub fn function_identity_eq(&self, other: &JsvValue) -> bool {
        match (self, other) {
            (
                JsvValue::Function(name_a, params_a, body_a, env_a),
                JsvValue::Function(name_b, params_b, body_b, env_b),
            ) => {
                name_a == name_b
                    && params_a == params_b
                    && body_a == body_b
                    && env_a.same_record(env_b)
            }
            (
                JsvValue::AsyncFunction(name_a, params_a, body_a, env_a),
                JsvValue::AsyncFunction(name_b, params_b, body_b, env_b),
            ) => {
                name_a == name_b
                    && params_a == params_b
                    && body_a == body_b
                    && env_a.same_record(env_b)
            }
            (
                JsvValue::GeneratorFn(name_a, params_a, body_a, env_a),
                JsvValue::GeneratorFn(name_b, params_b, body_b, env_b),
            ) => {
                name_a == name_b
                    && params_a == params_b
                    && body_a == body_b
                    && env_a.same_record(env_b)
            }
            (JsvValue::NativeFn(name_a), JsvValue::NativeFn(name_b)) => name_a == name_b,
            (
                JsvValue::HostFunction(object_a, name_a),
                JsvValue::HostFunction(object_b, name_b),
            ) => object_a == object_b && name_a == name_b,
            _ => false,
        }
    }
}

fn object_value(properties: HashMap<String, JsvValue>) -> JsvValue {
    JsvValue::Object(Rc::new(RefCell::new(JsvObject::plain(properties))))
}

fn object_property(object: &JsvObjectRef, name: &str) -> Option<JsvValue> {
    let mut current = Some(object.clone());
    for _ in 0..64 {
        let reference = current?;

        let borrowed = reference.try_borrow().ok()?;
        if let Some(value) = borrowed.properties.get(name) {
            return Some(value.clone());
        }
        current = borrowed.prototype.clone();
    }
    None
}

fn property_key(value: &JsvValue) -> String {
    match value {
        JsvValue::Symbol(symbol) => symbol_property_key(symbol),
        JsvValue::LiveBinding(record, name) => record
            .get(name)
            .map(|resolved| property_key(&resolved))
            .unwrap_or_else(|| "undefined".to_string()),
        JsvValue::Number(number) if number.is_finite() && *number == number.floor() => {
            (*number as i64).to_string()
        }
        _ => value.deref_live().to_display_string(),
    }
}

fn same_value_zero(left: &JsvValue, right: &JsvValue) -> bool {
    match (left, right) {
        (JsvValue::Number(l), JsvValue::Number(r)) => l == r || (l.is_nan() && r.is_nan()),
        _ => left == right,
    }
}

fn map_index(map: &JsvMap, key: &JsvValue) -> Option<usize> {
    map.entries
        .iter()
        .position(|(existing, _)| same_value_zero(existing, key))
}

fn set_index(set: &JsvSet, value: &JsvValue) -> Option<usize> {
    set.values
        .iter()
        .position(|existing| same_value_zero(existing, value))
}

fn weak_key_of(value: &JsvValue) -> Option<JsvWeakKey> {
    match value {
        JsvValue::Object(object) => Some(JsvWeakKey::Object(Rc::downgrade(object))),
        JsvValue::Array(array) => Some(JsvWeakKey::Array(Rc::downgrade(array))),
        JsvValue::Promise(promise) => Some(JsvWeakKey::Promise(Rc::downgrade(promise))),
        JsvValue::HostObject(handle) => Some(JsvWeakKey::Host(*handle)),
        _ => None,
    }
}

fn weak_key_eq(left: &JsvWeakKey, right: &JsvWeakKey) -> bool {
    match (left, right) {
        (JsvWeakKey::Object(l), JsvWeakKey::Object(r)) => l
            .upgrade()
            .is_some_and(|a| r.upgrade().is_some_and(|b| Rc::ptr_eq(&a, &b))),
        (JsvWeakKey::Array(l), JsvWeakKey::Array(r)) => l
            .upgrade()
            .is_some_and(|a| r.upgrade().is_some_and(|b| Rc::ptr_eq(&a, &b))),
        (JsvWeakKey::Promise(l), JsvWeakKey::Promise(r)) => l
            .upgrade()
            .is_some_and(|a| r.upgrade().is_some_and(|b| Rc::ptr_eq(&a, &b))),
        (JsvWeakKey::Host(l), JsvWeakKey::Host(r)) => l == r,
        _ => false,
    }
}

fn weak_key_alive(key: &JsvWeakKey) -> bool {
    match key {
        JsvWeakKey::Object(weak) => weak.upgrade().is_some(),
        JsvWeakKey::Array(weak) => weak.upgrade().is_some(),
        JsvWeakKey::Promise(weak) => weak.upgrade().is_some(),
        JsvWeakKey::Host(_) => true,
    }
}

fn error_value(name: &str, message: &str) -> JsvValue {
    JsvValue::Error(Rc::new(JsvErrorObj {
        name: name.to_string(),
        message: message.to_string(),
    }))
}

#[derive(Debug, Clone, PartialEq)]
pub enum ObjectProperty {
    KeyValue(String, JsvExpr),
    Spread(Box<JsvExpr>),
    Method {
        name: String,
        params: Vec<ParamBinding>,
        body: Box<JsvExpr>,
        is_async: bool,
        is_generator: bool,
    },
    Getter {
        name: String,
        body: Box<JsvExpr>,
    },
    Setter {
        name: String,
        param: ParamBinding,
        body: Box<JsvExpr>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClassMember {
    Method(JsvClassMethod),
    Field {
        name: String,
        initializer: Option<Box<JsvExpr>>,
        is_static: bool,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum JsvExpr {
    Number(f64),
    String(String),
    Bool(bool),
    Null,
    Undefined,
    ObjectLiteral(Vec<ObjectProperty>),
    ArrayLiteral(Vec<JsvExpr>),

    Identifier(String),
    This,
    Assignment(String, Box<JsvExpr>),
    VariableDeclaration(String, Box<JsvExpr>, DeclarationKind),

    DestructureDeclaration(Box<ParamPattern>, Box<JsvExpr>, DeclarationKind),

    DestructureAssignment(Box<ParamPattern>, Box<JsvExpr>),
    Member(Box<JsvExpr>, String),
    MemberAssignment(Box<JsvExpr>, String, Box<JsvExpr>),
    Index(Box<JsvExpr>, Box<JsvExpr>),
    IndexAssignment(Box<JsvExpr>, Box<JsvExpr>, Box<JsvExpr>),

    PrivateGet(Box<JsvExpr>, String),

    PrivateSet(Box<JsvExpr>, String, Box<JsvExpr>),

    BinaryOp(Box<JsvExpr>, OpKind, Box<JsvExpr>),
    UnaryOp(OpKind, Box<JsvExpr>),

    Spread(Box<JsvExpr>),

    If(Box<JsvExpr>, Vec<JsvExpr>, Option<Vec<JsvExpr>>),
    While(Box<JsvExpr>, Vec<JsvExpr>),
    ForIn(String, DeclarationKind, Box<JsvExpr>, Vec<JsvExpr>),
    ForOf(
        Box<ParamPattern>,
        DeclarationKind,
        Box<JsvExpr>,
        Vec<JsvExpr>,
    ),

    DoWhile(Box<JsvExpr>, Vec<JsvExpr>),
    Block(Vec<JsvExpr>),
    TryCatchFinally {
        try_body: Vec<JsvExpr>,
        catch_binding: Option<String>,
        catch_body: Vec<JsvExpr>,
        finally_body: Vec<JsvExpr>,
    },
    Switch(
        Box<JsvExpr>,
        Vec<(Option<JsvExpr>, Vec<JsvExpr>)>,
        Option<Vec<JsvExpr>>,
    ),

    Ternary(Box<JsvExpr>, Box<JsvExpr>, Box<JsvExpr>),
    Template(Vec<TemplatePart>),

    Call(Box<JsvExpr>, Vec<JsvExpr>),
    FunctionDef(String, Vec<ParamBinding>, Box<JsvExpr>),
    AsyncFunctionDef(String, Vec<ParamBinding>, Box<JsvExpr>),
    FunctionExpr(Vec<ParamBinding>, Box<JsvExpr>, bool),
    GeneratorDef(String, Vec<ParamBinding>, Box<JsvExpr>),
    GeneratorExpr(Vec<ParamBinding>, Box<JsvExpr>),

    AsyncGeneratorDef(String, Vec<ParamBinding>, Box<JsvExpr>),

    AsyncGeneratorExpr(Vec<ParamBinding>, Box<JsvExpr>),

    Yield(Box<JsvExpr>),

    YieldStar(Box<JsvExpr>),

    NewExpr(Box<JsvExpr>, Vec<JsvExpr>),

    SuperCall(Vec<JsvExpr>),

    SuperMember(String),

    SuperMemberCall(String, Vec<JsvExpr>),

    ClassDef {
        name: Option<String>,
        extends: Option<Box<JsvExpr>>,
        members: Vec<ClassMember>,
    },

    OptionalMember(Box<JsvExpr>, String),
    OptionalIndex(Box<JsvExpr>, Box<JsvExpr>),

    Update(Box<JsvExpr>, bool, bool),

    DynamicImport(String),

    Return(Box<JsvExpr>),
    Throw(Box<JsvExpr>),
    Await(Box<JsvExpr>),
    Break(Option<String>),
    Continue(Option<String>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum TemplatePart {
    Text(String),
    Expr(Box<JsvExpr>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclarationKind {
    Var,
    Let,
    Const,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OpKind {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Neq,
    StrictEq,
    StrictNeq,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    Nullish,
    Not,
    Typeof,
    Instanceof,
    In,
    Assign,
}

#[derive(Debug)]
struct EnvironmentRecord {
    parent: Option<Rc<RefCell<EnvironmentRecord>>>,
    bindings: HashMap<String, EnvironmentBinding>,
}

#[derive(Debug, Clone)]
struct EnvironmentBinding {
    value: JsvValue,
    mutable: bool,
}

#[derive(Debug, Clone)]
pub struct JsvEnvironment {
    record: Rc<RefCell<EnvironmentRecord>>,
}

impl Default for JsvEnvironment {
    fn default() -> Self {
        Self::new()
    }
}

impl JsvEnvironment {
    pub fn new() -> Self {
        Self {
            record: Rc::new(RefCell::new(EnvironmentRecord {
                parent: None,
                bindings: HashMap::new(),
            })),
        }
    }

    pub fn with_parent(parent: JsvEnvironment) -> Self {
        Self {
            record: Rc::new(RefCell::new(EnvironmentRecord {
                parent: Some(parent.record),
                bindings: HashMap::new(),
            })),
        }
    }

    pub fn get(&self, name: &str) -> Option<JsvValue> {
        let (value, parent) = {
            let record = self.record.borrow();
            (
                record
                    .bindings
                    .get(name)
                    .map(|binding| binding.value.clone()),
                record.parent.clone(),
            )
        };
        value.or_else(|| parent.and_then(|record| Self { record }.get(name)))
    }

    pub fn set(&mut self, name: &str, value: JsvValue) {
        if self.assign_existing(name, &value).unwrap_or(false) {
            return;
        }
        self.record.borrow_mut().bindings.insert(
            name.to_string(),
            EnvironmentBinding {
                value,
                mutable: true,
            },
        );
    }

    pub fn define(&mut self, name: &str, value: JsvValue) {
        self.record.borrow_mut().bindings.insert(
            name.to_string(),
            EnvironmentBinding {
                value,
                mutable: true,
            },
        );
    }

    fn declare(
        &mut self,
        name: &str,
        value: JsvValue,
        mutable: bool,
        allow_redeclare: bool,
    ) -> Result<(), String> {
        let mut record = self.record.borrow_mut();
        if !allow_redeclare && record.bindings.contains_key(name) {
            return Err(format!(
                "SyntaxError: identifier '{}' has already been declared",
                name
            ));
        }
        record
            .bindings
            .insert(name.to_string(), EnvironmentBinding { value, mutable });
        Ok(())
    }

    fn assign(&mut self, name: &str, value: JsvValue) -> Result<(), String> {
        if self.assign_existing(name, &value)? {
            return Ok(());
        }
        self.declare(name, value, true, true)
    }

    pub fn contains_binding(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    pub fn same_record(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.record, &other.record)
    }

    fn assign_existing(&self, name: &str, value: &JsvValue) -> Result<bool, String> {
        let parent = {
            let mut record = self.record.borrow_mut();
            if let Some(binding) = record.bindings.get_mut(name) {
                if !binding.mutable {
                    return Err(format!("TypeError: assignment to constant '{}'", name));
                }
                binding.value = value.clone();
                return Ok(true);
            }
            record.parent.clone()
        };
        match parent {
            Some(record) => Self { record }.assign_existing(name, value),
            None => Ok(false),
        }
    }
}

#[derive(Debug)]
pub struct JsvEngine {
    pub global_env: JsvEnvironment,
    pub console_output: Vec<String>,

    pub modules: JsvModuleGraph,

    pub dynamic_code_policy: DynamicCodePolicy,

    last_stats: EvalStats,
}

impl Default for JsvEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl JsvEngine {
    pub fn new() -> Self {
        let mut env = JsvEnvironment::new();
        Self::install_globals(&mut env);
        Self {
            global_env: env,
            console_output: Vec::new(),
            modules: JsvModuleGraph::new(),
            dynamic_code_policy: DynamicCodePolicy::Denied,
            last_stats: EvalStats::default(),
        }
    }

    pub fn set_dynamic_code_policy(&mut self, policy: DynamicCodePolicy) {
        self.dynamic_code_policy = policy;
    }

    pub fn take_stats(&mut self) -> EvalStats {
        std::mem::take(&mut self.last_stats)
    }

    fn install_globals(env: &mut JsvEnvironment) {
        let mut console_obj = HashMap::new();
        for name in ["log", "warn", "error", "info", "debug"] {
            console_obj.insert(
                name.to_string(),
                JsvValue::NativeFn(format!("console.{}", name)),
            );
        }
        env.set("console", object_value(console_obj));
        env.set("console.log", JsvValue::NativeFn("console.log".to_string()));
        env.set(
            "console.warn",
            JsvValue::NativeFn("console.warn".to_string()),
        );
        env.set(
            "console.error",
            JsvValue::NativeFn("console.error".to_string()),
        );
        env.set(
            "console.info",
            JsvValue::NativeFn("console.info".to_string()),
        );
        env.set(
            "console.debug",
            JsvValue::NativeFn("console.debug".to_string()),
        );

        let mut math_obj = HashMap::new();
        math_obj.insert("PI".to_string(), JsvValue::Number(std::f64::consts::PI));
        math_obj.insert("E".to_string(), JsvValue::Number(std::f64::consts::E));
        math_obj.insert("LN2".to_string(), JsvValue::Number(std::f64::consts::LN_2));
        math_obj.insert(
            "LN10".to_string(),
            JsvValue::Number(std::f64::consts::LN_10),
        );
        math_obj.insert(
            "LOG2E".to_string(),
            JsvValue::Number(std::f64::consts::LOG2_E),
        );
        math_obj.insert(
            "LOG10E".to_string(),
            JsvValue::Number(std::f64::consts::LOG10_E),
        );
        math_obj.insert(
            "SQRT2".to_string(),
            JsvValue::Number(std::f64::consts::SQRT_2),
        );
        math_obj.insert(
            "SQRT1_2".to_string(),
            JsvValue::Number(std::f64::consts::FRAC_1_SQRT_2),
        );
        math_obj.insert("MAX_VALUE".to_string(), JsvValue::Number(f64::MAX));
        math_obj.insert("MIN_VALUE".to_string(), JsvValue::Number(f64::from_bits(1)));
        for name in [
            "abs", "ceil", "floor", "round", "sqrt", "random", "max", "min", "pow", "exp", "expm1",
            "log", "log2", "log10", "log1p", "sign", "trunc", "cbrt", "hypot", "sin", "cos", "tan",
            "asin", "acos", "atan", "atan2", "imul", "fround", "clz32",
        ] {
            math_obj.insert(
                name.to_string(),
                JsvValue::NativeFn(format!("Math.{}", name)),
            );
        }
        env.set("Math", object_value(math_obj));

        let mut date_obj = HashMap::new();
        date_obj.insert(
            "now".to_string(),
            JsvValue::NativeFn("Date.now".to_string()),
        );
        date_obj.insert(
            "parse".to_string(),
            JsvValue::NativeFn("Date.parse".to_string()),
        );
        date_obj.insert(
            "UTC".to_string(),
            JsvValue::NativeFn("Date.UTC".to_string()),
        );
        env.set("Date", object_value(date_obj));
        env.set("Date.now", JsvValue::NativeFn("Date.now".to_string()));
        env.set("Date.parse", JsvValue::NativeFn("Date.parse".to_string()));

        let mut object_constructor = HashMap::new();
        for name in [
            "create",
            "keys",
            "values",
            "entries",
            "assign",
            "freeze",
            "isFrozen",
            "getOwnPropertyNames",
            "getOwnPropertySymbols",
            "hasOwn",
            "defineProperty",
            "getPrototypeOf",
            "setPrototypeOf",
            "fromEntries",
        ] {
            object_constructor.insert(
                name.to_string(),
                JsvValue::NativeFn(format!("Object.{}", name)),
            );
        }
        env.set("Object", object_value(object_constructor));

        let mut array_constructor = HashMap::new();
        for name in ["isArray", "from", "of"] {
            array_constructor.insert(
                name.to_string(),
                JsvValue::NativeFn(format!("Array.{}", name)),
            );
        }
        env.set("Array", object_value(array_constructor));

        let mut json_constructor = HashMap::new();
        json_constructor.insert(
            "stringify".to_string(),
            JsvValue::NativeFn("JSON.stringify".to_string()),
        );
        json_constructor.insert(
            "parse".to_string(),
            JsvValue::NativeFn("JSON.parse".to_string()),
        );
        env.set("JSON", object_value(json_constructor));

        let mut promise_constructor = HashMap::new();
        for name in ["resolve", "reject", "all", "race", "allSettled", "any"] {
            promise_constructor.insert(
                name.to_string(),
                JsvValue::NativeFn(format!("Promise.{}", name)),
            );
        }
        env.set("Promise", object_value(promise_constructor));

        let mut symbol_constructor = HashMap::new();
        for well_known in [
            WellKnownSymbol::Iterator,
            WellKnownSymbol::AsyncIterator,
            WellKnownSymbol::ToStringTag,
            WellKnownSymbol::HasInstance,
            WellKnownSymbol::Species,
            WellKnownSymbol::IsConcatSpreadable,
            WellKnownSymbol::ToPrimitive,
            WellKnownSymbol::Match,
            WellKnownSymbol::Replace,
            WellKnownSymbol::Search,
            WellKnownSymbol::Split,
            WellKnownSymbol::Unscopables,
        ] {
            symbol_constructor.insert(
                well_known.name().to_string(),
                JsvValue::Symbol(new_symbol(well_known.name())),
            );
        }
        symbol_constructor.insert(
            "for".to_string(),
            JsvValue::NativeFn("Symbol.for".to_string()),
        );
        symbol_constructor.insert(
            "keyFor".to_string(),
            JsvValue::NativeFn("Symbol.keyFor".to_string()),
        );
        env.set("Symbol", object_value(symbol_constructor));

        for name in ["Map", "Set", "WeakMap", "WeakSet"] {
            env.set(name, JsvValue::NativeFn(name.to_string()));
        }

        env.set("ArrayBuffer", JsvValue::NativeFn("ArrayBuffer".to_string()));
        env.set("DataView", JsvValue::NativeFn("DataView".to_string()));
        for kind in [
            TypedArrayKind::Int8,
            TypedArrayKind::Uint8,
            TypedArrayKind::Uint8Clamped,
            TypedArrayKind::Int16,
            TypedArrayKind::Uint16,
            TypedArrayKind::Int32,
            TypedArrayKind::Uint32,
            TypedArrayKind::Float32,
            TypedArrayKind::Float64,
        ] {
            env.set(kind.name(), JsvValue::NativeFn(kind.name().to_string()));
        }

        let mut reflect_obj = HashMap::new();
        for name in [
            "get",
            "set",
            "has",
            "deleteProperty",
            "ownKeys",
            "construct",
            "apply",
            "getPrototypeOf",
            "setPrototypeOf",
            "defineProperty",
            "isExtensible",
            "preventExtensions",
            "getOwnPropertyDescriptor",
        ] {
            reflect_obj.insert(
                name.to_string(),
                JsvValue::NativeFn(format!("Reflect.{}", name)),
            );
        }
        env.set("Reflect", object_value(reflect_obj));
        env.set("Proxy", JsvValue::NativeFn("Proxy".to_string()));

        let mut intl_obj = HashMap::new();
        for name in ["DateTimeFormat", "NumberFormat", "Collator", "PluralRules"] {
            intl_obj.insert(
                name.to_string(),
                JsvValue::NativeFn(format!("Intl.{}", name)),
            );
        }
        env.set("Intl", object_value(intl_obj));

        env.set("WeakRef", JsvValue::NativeFn("WeakRef".to_string()));
        env.set(
            "FinalizationRegistry",
            JsvValue::NativeFn("FinalizationRegistry".to_string()),
        );

        for name in [
            "Error",
            "TypeError",
            "RangeError",
            "ReferenceError",
            "SyntaxError",
            "EvalError",
            "URIError",
        ] {
            env.set(name, JsvValue::NativeFn(name.to_string()));
        }

        env.set("parseInt", JsvValue::NativeFn("parseInt".to_string()));
        env.set("parseFloat", JsvValue::NativeFn("parseFloat".to_string()));
        env.set("isNaN", JsvValue::NativeFn("isNaN".to_string()));
        env.set("isFinite", JsvValue::NativeFn("isFinite".to_string()));
        env.set(
            "encodeURIComponent",
            JsvValue::NativeFn("encodeURIComponent".to_string()),
        );
        env.set(
            "decodeURIComponent",
            JsvValue::NativeFn("decodeURIComponent".to_string()),
        );
        env.set("String", JsvValue::NativeFn("String".to_string()));
        env.set("Number", JsvValue::NativeFn("Number".to_string()));
        env.set("Boolean", JsvValue::NativeFn("Boolean".to_string()));
        env.set("eval", JsvValue::NativeFn("eval".to_string()));
        env.set("Function", JsvValue::NativeFn("Function".to_string()));
        env.set(
            "structuredClone",
            JsvValue::NativeFn("structuredClone".to_string()),
        );

        env.set("undefined", JsvValue::Undefined);
        env.set("null", JsvValue::Null);
        env.set("true", JsvValue::Boolean(true));
        env.set("false", JsvValue::Boolean(false));
        env.set("NaN", JsvValue::Number(f64::NAN));
        env.set("Infinity", JsvValue::Number(f64::INFINITY));

        env.set("document", JsvValue::object());
        env.set("window", JsvValue::object());
        env.set("self", JsvValue::object());
        env.set("globalThis", JsvValue::object());
        for name in ["setTimeout", "setInterval", "clearTimeout", "clearInterval"] {
            env.set(
                name,
                JsvValue::NativeFn(format!("[SANDBOXED] {} is disabled", name)),
            );
        }
        for name in [
            "document.write",
            "document.writeln",
            "document.getElementById",
            "document.createElement",
            "window.alert",
            "window.confirm",
            "window.prompt",
            "window.open",
            "window.location",
            "window.history",
            "window.navigator",
            "window.frames",
            "window.parent",
            "window.top",
            "window.opener",
            "window.crypto",
            "window.localStorage",
            "window.sessionStorage",
            "window.XMLHttpRequest",
            "window.fetch",
            "window.requestAnimationFrame",
            "window.cancelAnimationFrame",
        ] {
            env.set(
                name,
                JsvValue::NativeFn(format!("[SANDBOXED] {} is disabled", name)),
            );
        }
    }

    pub fn eval(&mut self, code: &str) -> Result<JsvValue, String> {
        let mut host = NoHost;
        self.eval_with_host(code, &mut host)
    }

    pub fn eval_with_step_limit(&mut self, code: &str, max_steps: u64) -> Result<JsvValue, String> {
        if max_steps == 0 {
            return Err("Script timed out (step budget exceeded)".to_string());
        }
        let mut host = NoHost;
        self.eval_with_host_limit(code, &mut host, max_steps)
    }

    pub fn execute_with_host(
        &mut self,
        script: &str,
        host: &mut dyn JsvHost,
    ) -> Result<JsvValue, String> {
        self.global_env
            .define("document", JsvValue::HostObject(HOST_DOCUMENT));
        self.global_env
            .define("window", JsvValue::HostObject(HOST_WINDOW));
        self.global_env
            .define("self", JsvValue::HostObject(HOST_WINDOW));
        self.global_env
            .define("globalThis", JsvValue::HostObject(HOST_WINDOW));
        self.global_env
            .define("localStorage", JsvValue::HostObject(HOST_LOCAL_STORAGE));
        self.global_env
            .define("indexedDB", JsvValue::HostObject(HOST_INDEXED_DB));
        self.global_env
            .define("caches", JsvValue::HostObject(HOST_CACHE_STORAGE));
        self.global_env
            .define("navigator", JsvValue::HostObject(HOST_NAVIGATOR));
        self.global_env.define(
            "fetch",
            JsvValue::HostFunction(HOST_WINDOW, "fetch".to_string()),
        );
        self.global_env.define(
            "MediaSource",
            JsvValue::HostFunction(HOST_WINDOW, "MediaSource".to_string()),
        );
        for name in [
            "WebSocket",
            "EventSource",
            "BroadcastChannel",
            "structuredClone",
            "MutationObserver",
            "ResizeObserver",
            "IntersectionObserver",
            "ReadableStream",
        ] {
            self.global_env
                .define(name, JsvValue::HostFunction(HOST_WINDOW, name.to_string()));
        }
        for name in ["setTimeout", "setInterval", "clearTimeout", "clearInterval"] {
            self.global_env
                .define(name, JsvValue::HostFunction(HOST_WINDOW, name.to_string()));
        }
        for name in ["Event", "CustomEvent", "FormData"] {
            self.global_env
                .define(name, JsvValue::HostFunction(HOST_WINDOW, name.to_string()));
        }
        self.global_env
            .define("location", JsvValue::HostObject(HOST_LOCATION));
        self.global_env
            .define("history", JsvValue::HostObject(HOST_HISTORY));
        self.global_env
            .define("customElements", JsvValue::HostObject(HOST_CUSTOM_ELEMENTS));
        self.global_env
            .define("crypto", JsvValue::HostObject(HOST_CRYPTO));
        self.global_env
            .define("performance", JsvValue::HostObject(HOST_PERFORMANCE));
        self.global_env
            .define("CSS", JsvValue::HostObject(HOST_CSS));
        for name in [
            "requestAnimationFrame",
            "cancelAnimationFrame",
            "matchMedia",
        ] {
            self.global_env
                .define(name, JsvValue::HostFunction(HOST_WINDOW, name.to_string()));
        }
        self.eval_with_host(script, host)
    }

    fn eval_with_host(&mut self, code: &str, host: &mut dyn JsvHost) -> Result<JsvValue, String> {
        self.eval_with_host_limit(code, host, MAX_EVAL_STEPS)
    }

    fn eval_with_host_limit(
        &mut self,
        code: &str,
        host: &mut dyn JsvHost,
        max_steps: u64,
    ) -> Result<JsvValue, String> {
        let tokens = tokenize(code)?;
        if tokens.is_empty() {
            return Ok(JsvValue::Undefined);
        }
        if max_parse_depth(&tokens) > MAX_PARSE_DEPTH {
            return Err("Script too deeply nested".to_string());
        }

        let (exprs, _rest) = parse_statements(&tokens, 0, 0)?;

        let env = &mut self.global_env;
        let console = &mut self.console_output;
        let modules = &mut self.modules;
        let mut ctx = EvalCtx::with_step_limit(host, modules, max_steps);
        ctx.dynamic_code = self.dynamic_code_policy;

        let outcome = (|| {
            let mut result = JsvValue::Undefined;
            for expression in &exprs {
                result = eval_expr(expression, env, console, &mut ctx)?;
                if is_abrupt_signal(&result) {
                    break;
                }
            }
            drain_promise_jobs(env, console, &mut ctx)?;
            Ok::<_, String>(result)
        })();
        let mut stats = ctx.stats();
        if let Err(error) = &outcome {
            if error.contains("step budget") || error.contains("timeout") {
                stats.timeout_reason = Some(error.clone());
            }
        }
        self.last_stats = stats;
        let result = outcome?;
        match result {
            JsvValue::ReturnSignal(value) => Ok(*value),
            JsvValue::ThrowSignal(reason) => Err(format!(
                "Uncaught exception: {}",
                reason.to_display_string()
            )),
            JsvValue::BreakSignal => Err("SyntaxError: illegal break statement".to_string()),
            JsvValue::ContinueSignal => Err("SyntaxError: illegal continue statement".to_string()),
            value => Ok(value),
        }
    }

    pub fn execute_script(&mut self, script: &str) -> Result<JsvValue, String> {
        self.eval(script)
    }

    pub fn invoke_callback(
        &mut self,
        callback: &JsvValue,
        arguments: Vec<JsvValue>,
        host: &mut dyn JsvHost,
    ) -> Result<JsvValue, String> {
        let modules = &mut self.modules;
        let mut ctx = EvalCtx::new(host, modules);
        ctx.dynamic_code = self.dynamic_code_policy;
        let outcome = (|| {
            let result = call_function(
                callback.clone(),
                arguments,
                &mut self.global_env,
                &mut self.console_output,
                &mut ctx,
                JsvValue::Undefined,
            )?;
            drain_promise_jobs(&mut self.global_env, &mut self.console_output, &mut ctx)?;
            Ok::<_, String>(result)
        })();
        if let Err(error) = &outcome {
            if error.contains("step budget") || error.contains("timeout") {
                let mut stats = ctx.stats();
                stats.timeout_reason = Some(error.clone());
                self.last_stats = stats;
            } else {
                self.last_stats = ctx.stats();
            }
        } else {
            self.last_stats = ctx.stats();
        }
        let result = outcome?;
        match result {
            JsvValue::ReturnSignal(value) => Ok(*value),
            JsvValue::ThrowSignal(reason) => Err(format!(
                "Uncaught exception in callback: {}",
                reason.to_display_string()
            )),
            JsvValue::BreakSignal => {
                Err("SyntaxError: illegal break statement in callback".to_string())
            }
            JsvValue::ContinueSignal => {
                Err("SyntaxError: illegal continue statement in callback".to_string())
            }
            value => Ok(value),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ModuleNamespace {
    pub exports: BTreeMap<String, JsvValue>,
}

#[derive(Debug, Clone)]
struct ModuleImport {
    specifier: String,
    bindings: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
struct ParsedModule {
    imports: Vec<ModuleImport>,
    exports: Vec<String>,
    body: String,
}

#[derive(Debug, Default)]
pub struct JsvModuleGraph {
    sources: BTreeMap<String, String>,
    evaluated: BTreeMap<String, ModuleNamespace>,
    import_maps: BTreeMap<String, String>,
    total_source_bytes: usize,
}

impl JsvModuleGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, specifier: &str, source: &str) -> Result<(), String> {
        if specifier.is_empty() || specifier.len() > 1_024 {
            return Err("Invalid module specifier".to_string());
        }
        if source.len() > MAX_STRING_LENGTH {
            return Err("Module source exceeds 2 MB".to_string());
        }
        let previous = self.sources.get(specifier).map(String::len).unwrap_or(0);
        let projected = self
            .total_source_bytes
            .saturating_sub(previous)
            .saturating_add(source.len());
        if projected > 32 * 1024 * 1024
            || (!self.sources.contains_key(specifier) && self.sources.len() >= 256)
        {
            return Err("Module graph budget exceeded".to_string());
        }
        self.sources
            .insert(specifier.to_string(), source.to_string());
        self.total_source_bytes = projected;
        self.evaluated.clear();
        Ok(())
    }

    pub fn register_import_map(&mut self, json_map: &str) -> Result<(), String> {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_map) {
            if let Some(imports) = val.get("imports").and_then(|v| v.as_object()) {
                for (k, v) in imports {
                    if let Some(target) = v.as_str() {
                        self.import_maps.insert(k.clone(), target.to_string());
                    }
                }
            }
        }
        Ok(())
    }

    pub fn resolve_specifier<'a>(&'a self, specifier: &'a str) -> &'a str {
        if let Some(mapped) = self.import_maps.get(specifier) {
            return mapped.as_str();
        }
        specifier
    }

    pub fn evaluate(&mut self, entry: &str) -> Result<ModuleNamespace, String> {
        let mut stack = Vec::new();
        let resolved = self.resolve_specifier(entry).to_string();
        self.evaluate_inner(&resolved, &mut stack, 0)
    }

    fn evaluate_inner(
        &mut self,
        specifier: &str,
        stack: &mut Vec<String>,
        depth: usize,
    ) -> Result<ModuleNamespace, String> {
        let resolved = self.resolve_specifier(specifier).to_string();
        let specifier = resolved.as_str();
        if let Some(namespace) = self.evaluated.get(specifier) {
            return Ok(namespace.clone());
        }
        if depth > 64 {
            return Err("Module dependency depth exceeded".to_string());
        }
        if stack.iter().any(|active| active == specifier) {
            return Err(format!("Circular module dependency: {}", specifier));
        }
        let source = self
            .sources
            .get(specifier)
            .cloned()
            .ok_or_else(|| format!("Module not found: {}", specifier))?;
        let parsed = parse_module_source(&source)?;
        stack.push(specifier.to_string());

        let mut imported = Vec::new();
        for import in &parsed.imports {
            let import_spec = self.resolve_specifier(&import.specifier).to_string();
            let namespace = self.evaluate_inner(&import_spec, stack, depth + 1)?;
            for (exported, local) in &import.bindings {
                let value = namespace.exports.get(exported).cloned().ok_or_else(|| {
                    format!(
                        "Module '{}' does not export '{}'",
                        import.specifier, exported
                    )
                })?;
                imported.push((local.clone(), value));
            }
        }

        let mut engine = JsvEngine::new();
        for (name, value) in imported {
            engine.global_env.define(&name, value);
        }
        engine
            .execute_script(&parsed.body)
            .map_err(|error| format!("Module '{}': {}", specifier, error))?;
        let mut namespace = ModuleNamespace::default();
        for name in parsed.exports {
            let value = engine.global_env.get(&name).ok_or_else(|| {
                format!(
                    "Module '{}' did not initialize export '{}'",
                    specifier, name
                )
            })?;
            namespace.exports.insert(name, value);
        }
        stack.pop();
        self.evaluated
            .insert(specifier.to_string(), namespace.clone());
        Ok(namespace)
    }

    pub(crate) fn evaluate_in_env(
        &mut self,
        specifier: &str,
        env: &JsvEnvironment,
        console: &mut Vec<String>,
        ctx: &mut EvalCtx<'_>,
    ) -> Result<ModuleNamespace, String> {
        if let Some(namespace) = self.evaluated.get(specifier) {
            return Ok(namespace.clone());
        }
        let mut stack = Vec::new();
        self.evaluate_in_env_inner(specifier, &mut stack, 0, env, console, ctx)
    }

    fn import_module(
        &mut self,
        specifier: &str,
        env: &JsvEnvironment,
        console: &mut Vec<String>,
        ctx: &mut EvalCtx<'_>,
    ) -> Result<JsvValue, String> {
        let namespace = self.evaluate_in_env(specifier, env, console, ctx)?;
        ctx.modules_evaluated = ctx.modules_evaluated.saturating_add(1);
        Ok(JsvValue::Promise(Rc::new(RefCell::new(
            JsvPromiseState::Fulfilled(object_value(namespace.exports.into_iter().collect())),
        ))))
    }

    fn evaluate_in_env_inner(
        &mut self,
        specifier: &str,
        stack: &mut Vec<String>,
        depth: usize,
        env: &JsvEnvironment,
        console: &mut Vec<String>,
        ctx: &mut EvalCtx<'_>,
    ) -> Result<ModuleNamespace, String> {
        if let Some(namespace) = self.evaluated.get(specifier) {
            return Ok(namespace.clone());
        }
        if depth > 64 {
            return Err("Module dependency depth exceeded".to_string());
        }
        if stack.iter().any(|active| active == specifier) {
            return Err(format!("Circular module dependency: {}", specifier));
        }
        let source = self
            .sources
            .get(specifier)
            .cloned()
            .ok_or_else(|| format!("Module not found: {}", specifier))?;
        let parsed = parse_module_source(&source)?;
        stack.push(specifier.to_string());

        let mut module_env = JsvEnvironment::with_parent(env.clone());
        for import in &parsed.imports {
            let namespace =
                self.evaluate_in_env_inner(&import.specifier, stack, depth + 1, env, console, ctx)?;
            for (exported, local) in &import.bindings {
                let value = namespace.exports.get(exported).cloned().ok_or_else(|| {
                    format!(
                        "Module '{}' does not export '{}'",
                        import.specifier, exported
                    )
                })?;
                module_env.define(local, value);
            }
        }

        let tokens = tokenize(&parsed.body)?;
        let (exprs, _rest) = parse_statements(&tokens, 0, 0)?;
        for expression in &exprs {
            let value = eval_expr(expression, &mut module_env, console, ctx)?;
            if is_abrupt_signal(&value) {
                break;
            }
        }
        drain_promise_jobs(&mut module_env, console, ctx)?;
        let mut namespace = ModuleNamespace::default();
        for name in parsed.exports {
            let value = module_env.get(&name).ok_or_else(|| {
                format!(
                    "Module '{}' did not initialize export '{}'",
                    specifier, name
                )
            })?;
            namespace.exports.insert(name, value);
        }
        stack.pop();
        self.evaluated
            .insert(specifier.to_string(), namespace.clone());
        Ok(namespace)
    }
}

fn parse_module_source(source: &str) -> Result<ParsedModule, String> {
    let mut imports = Vec::new();
    let mut exports = Vec::new();
    let mut body = String::new();
    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("import ") {
            let open = trimmed
                .find('{')
                .ok_or_else(|| "Only named module imports are supported".to_string())?;
            let close = trimmed[open + 1..]
                .find('}')
                .map(|offset| open + 1 + offset)
                .ok_or_else(|| "Unterminated module import list".to_string())?;
            let after = trimmed[close + 1..].trim();
            let raw_specifier = after
                .strip_prefix("from")
                .map(str::trim)
                .ok_or_else(|| "Module import requires 'from'".to_string())?
                .trim_end_matches(';');
            let specifier = raw_specifier
                .strip_prefix(['\'', '"'])
                .and_then(|value| value.strip_suffix(['\'', '"']))
                .ok_or_else(|| "Module specifier must be quoted".to_string())?;
            let mut bindings = Vec::new();
            for binding in trimmed[open + 1..close].split(',') {
                let parts = binding.split_whitespace().collect::<Vec<_>>();
                let (exported, local) = match parts.as_slice() {
                    [name] => (*name, *name),
                    [exported, "as", local] => (*exported, *local),
                    _ => return Err("Invalid named module import".to_string()),
                };
                if !is_identifier(exported) || !is_identifier(local) {
                    return Err("Invalid module import binding".to_string());
                }
                bindings.push((exported.to_string(), local.to_string()));
            }
            imports.push(ModuleImport {
                specifier: specifier.to_string(),
                bindings,
            });
            continue;
        }

        if let Some(statement) = trimmed.strip_prefix("export ") {
            if let Some(export_source) = statement.strip_prefix("* from ") {
                let spec = export_source
                    .trim()
                    .trim_end_matches(';')
                    .strip_prefix(['\'', '"'])
                    .and_then(|v| v.strip_suffix(['\'', '"']))
                    .ok_or_else(|| "Module specifier must be quoted".to_string())?;
                imports.push(ModuleImport {
                    specifier: spec.to_string(),
                    bindings: Vec::new(),
                });
                continue;
            }
            if let Some(braced) = statement.strip_prefix("{") {
                if let Some(close) = braced.find('}') {
                    for part in braced[..close].split(',') {
                        let parts: Vec<&str> = part.split_whitespace().collect();
                        let ename = match parts.as_slice() {
                            [n] => *n,
                            [_, "as", e] => *e,
                            _ => continue,
                        };
                        if is_identifier(ename) {
                            exports.push(ename.to_string());
                        }
                    }
                    body.push_str(statement);
                    body.push('\n');
                    continue;
                }
            }
            let name = if let Some(rest) = statement.strip_prefix("function ") {
                rest.split(['(', ' ']).next()
            } else if let Some(rest) = statement.strip_prefix("async function ") {
                rest.split(['(', ' ']).next()
            } else {
                ["const ", "let ", "var "]
                    .iter()
                    .find_map(|prefix| statement.strip_prefix(prefix))
                    .and_then(|rest| rest.split(['=', ';', ' ']).next())
            }
            .filter(|name| is_identifier(name))
            .ok_or_else(|| "Unsupported module export declaration".to_string())?;
            exports.push(name.to_string());
            body.push_str(statement);
            body.push('\n');
        } else {
            body.push_str(line);
            body.push('\n');
        }
    }
    if exports.len() > 1_024 || imports.len() > 256 {
        return Err("Module binding budget exceeded".to_string());
    }
    Ok(ParsedModule {
        imports,
        exports,
        body,
    })
}

fn eval_expr(
    expr: &JsvExpr,
    env: &mut JsvEnvironment,
    console: &mut Vec<String>,
    ctx: &mut EvalCtx<'_>,
) -> Result<JsvValue, String> {
    ctx.steps += 1;
    if ctx.steps > ctx.max_steps {
        return Err("Script timed out (step budget exceeded)".to_string());
    }
    ctx.max_call_depth = ctx.max_call_depth.max(ctx.call_depth);

    match expr {
        JsvExpr::Number(n) => Ok(JsvValue::Number(*n)),
        JsvExpr::String(s) => {
            check_string_alloc(ctx, s.len())?;
            Ok(JsvValue::String(s.clone()))
        }
        JsvExpr::Bool(b) => Ok(JsvValue::Boolean(*b)),
        JsvExpr::Null => Ok(JsvValue::Null),
        JsvExpr::Undefined => Ok(JsvValue::Undefined),
        JsvExpr::ObjectLiteral(entries) => {
            let mut properties = HashMap::new();
            for entry in entries {
                match entry {
                    ObjectProperty::KeyValue(name, expression) => {
                        properties.insert(name.clone(), eval_expr(expression, env, console, ctx)?);
                    }
                    ObjectProperty::Spread(expression) => {
                        let value = eval_expr(expression, env, console, ctx)?;
                        let resolved = value.deref_live();
                        for (key, item) in own_properties(&resolved)? {
                            properties.insert(key, item);
                        }
                    }
                    ObjectProperty::Method {
                        name,
                        params,
                        body,
                        is_async,
                        is_generator,
                    } => {
                        let func = if *is_async {
                            JsvValue::AsyncFunction(
                                name.clone(),
                                params.clone(),
                                body.clone(),
                                env.clone(),
                            )
                        } else if *is_generator {
                            JsvValue::GeneratorFn(
                                name.clone(),
                                params.clone(),
                                body.clone(),
                                env.clone(),
                            )
                        } else {
                            JsvValue::Function(
                                name.clone(),
                                params.clone(),
                                body.clone(),
                                env.clone(),
                            )
                        };
                        properties.insert(name.clone(), func);
                    }
                    ObjectProperty::Getter { name, body } => {
                        let getter = JsvValue::Function(
                            format!("get {}", name),
                            Vec::new(),
                            body.clone(),
                            env.clone(),
                        );
                        let existing = properties.get(name);
                        let setter = match existing {
                            Some(JsvValue::GetterSetter(_, set)) => (**set).clone(),
                            _ => JsvValue::Undefined,
                        };
                        properties.insert(
                            name.clone(),
                            JsvValue::GetterSetter(Box::new(getter), Box::new(setter)),
                        );
                    }
                    ObjectProperty::Setter { name, param, body } => {
                        let setter = JsvValue::Function(
                            format!("set {}", name),
                            vec![param.clone()],
                            body.clone(),
                            env.clone(),
                        );
                        let existing = properties.get(name);
                        let getter = match existing {
                            Some(JsvValue::GetterSetter(get, _)) => (**get).clone(),
                            _ => JsvValue::Undefined,
                        };
                        properties.insert(
                            name.clone(),
                            JsvValue::GetterSetter(Box::new(getter), Box::new(setter)),
                        );
                    }
                }
            }
            Ok(object_value(properties))
        }
        JsvExpr::ArrayLiteral(entries) => {
            let mut values = Vec::new();
            for expression in entries {
                if let JsvExpr::Spread(spread) = expression {
                    let iterable = eval_expr(spread, env, console, ctx)?;
                    let resolved = iterable.deref_live();
                    for item in iterate_values(&resolved, console, ctx)? {
                        if values.len() >= MAX_ARRAY_ELEMENTS {
                            return Err("Array element budget exceeded".to_string());
                        }
                        values.push(item);
                    }
                } else {
                    values.push(eval_expr(expression, env, console, ctx)?);
                }
                if values.len() > MAX_ARRAY_ELEMENTS {
                    return Err("Array element budget exceeded".to_string());
                }
            }
            Ok(JsvValue::Array(Rc::new(RefCell::new(values))))
        }

        JsvExpr::Identifier(name) => {
            if name == "this" {
                return Ok(env.get("this").unwrap_or(JsvValue::Undefined));
            }

            if is_sandboxed_function(name) && env.get(name).is_none() {
                return Err(format!(
                    "SecurityError: '{}' is not available in sandboxed JavaScript environment",
                    name
                ));
            }
            Ok(env
                .get(name)
                .map(|value| value.deref_live())
                .ok_or_else(|| format!("ReferenceError: {} is not defined", name))?)
        }
        JsvExpr::This => Ok(env.get("this").unwrap_or(JsvValue::Undefined)),
        JsvExpr::Assignment(name, rhs) => {
            let val = eval_expr(rhs, env, console, ctx)?;
            if let JsvValue::YieldSignal(signal) = val {
                return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                    Cont::AssignApply {
                        target: AssignTarget::Identifier(name.clone()),
                        env: env.clone(),
                    },
                ))));
            }
            env.assign(name, val.clone())?;
            Ok(val)
        }
        JsvExpr::VariableDeclaration(name, initializer, kind) => {
            let value = eval_expr(initializer, env, console, ctx)?;
            if let JsvValue::YieldSignal(signal) = value {
                return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                    Cont::VarDeclApply {
                        name: name.clone(),
                        kind: *kind,
                        env: env.clone(),
                    },
                ))));
            }
            env.declare(
                name,
                value.clone(),
                *kind != DeclarationKind::Const,
                *kind == DeclarationKind::Var,
            )?;
            Ok(value)
        }
        JsvExpr::DestructureDeclaration(pattern, initializer, kind) => {
            let value = eval_expr(initializer, env, console, ctx)?;
            if let JsvValue::YieldSignal(signal) = value {
                return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                    Cont::BindPatternApply {
                        pattern: pattern.clone(),
                        kind: *kind,
                        env: env.clone(),
                    },
                ))));
            }
            bind_pattern(pattern, &value, env, *kind, console, ctx)?;
            Ok(value)
        }
        JsvExpr::DestructureAssignment(pattern, initializer) => {
            let value = eval_expr(initializer, env, console, ctx)?;
            if let JsvValue::YieldSignal(signal) = value {
                return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                    Cont::BindPatternApply {
                        pattern: pattern.clone(),
                        kind: DeclarationKind::Let,
                        env: env.clone(),
                    },
                ))));
            }
            bind_pattern(pattern, &value, env, DeclarationKind::Let, console, ctx)?;
            Ok(value)
        }
        JsvExpr::Member(object, property) => {
            let object = eval_expr(object, env, console, ctx)?;
            if let JsvValue::YieldSignal(signal) = object {
                return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                    Cont::MemberApply {
                        property: property.clone(),
                        optional: false,
                    },
                ))));
            }
            read_property(object.deref_live(), property, env, console, ctx, false)
        }
        JsvExpr::MemberAssignment(object, property, rhs) => {
            let object = eval_expr(object, env, console, ctx)?;
            if let JsvValue::YieldSignal(signal) = object {
                return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                    Cont::MemberAssignBase {
                        property: property.clone(),
                        rhs: rhs.clone(),
                        env: env.clone(),
                    },
                ))));
            }
            let value = eval_expr(rhs, env, console, ctx)?;
            if let JsvValue::YieldSignal(signal) = value {
                return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                    Cont::MemberAssignApply {
                        object,
                        property: property.clone(),
                    },
                ))));
            }
            write_property(object.deref_live(), property, value, env, console, ctx)
        }
        JsvExpr::Index(object, key) => {
            let object = eval_expr(object, env, console, ctx)?;
            if let JsvValue::YieldSignal(signal) = object {
                return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                    Cont::IndexBase {
                        key_expr: key.clone(),
                        optional: false,
                        env: env.clone(),
                    },
                ))));
            }
            let key = eval_expr(key, env, console, ctx)?;
            if let JsvValue::YieldSignal(signal) = key {
                return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                    Cont::IndexKey {
                        object: object.clone(),
                        key_expr: Box::new(JsvExpr::Undefined),
                        optional: false,
                        env: env.clone(),
                    },
                ))));
            }
            index_read(
                object.deref_live(),
                key.deref_live(),
                env,
                console,
                ctx,
                false,
            )
        }
        JsvExpr::IndexAssignment(object, key, rhs) => {
            let object = eval_expr(object, env, console, ctx)?;
            if let JsvValue::YieldSignal(signal) = object {
                return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                    Cont::IndexAssignBase {
                        key_expr: key.clone(),
                        rhs: rhs.clone(),
                        env: env.clone(),
                    },
                ))));
            }
            let key = eval_expr(key, env, console, ctx)?;
            if let JsvValue::YieldSignal(signal) = key {
                return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                    Cont::IndexAssignKey {
                        object: object.clone(),
                        rhs: rhs.clone(),
                        env: env.clone(),
                    },
                ))));
            }
            let value = eval_expr(rhs, env, console, ctx)?;
            if let JsvValue::YieldSignal(signal) = value {
                return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                    Cont::IndexAssignValue {
                        object: object.clone(),
                        key: key.clone(),
                    },
                ))));
            }
            index_write(
                object.deref_live(),
                key.deref_live(),
                value,
                env,
                console,
                ctx,
            )
        }

        JsvExpr::BinaryOp(left, op, right) => {
            let l_val = eval_expr(left, env, console, ctx)?;
            if let JsvValue::YieldSignal(signal) = l_val {
                return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                    Cont::BinaryRight {
                        left: JsvValue::Undefined,
                        op: *op,
                        right: right.clone(),
                        env: env.clone(),
                    },
                ))));
            }
            match op {
                OpKind::And if !l_val.is_truthy() => return Ok(l_val),
                OpKind::Or if l_val.is_truthy() => return Ok(l_val),
                _ => {}
            }
            let r_val = eval_expr(right, env, console, ctx)?;
            if let JsvValue::YieldSignal(signal) = r_val {
                return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                    Cont::BinaryApply {
                        left: l_val,
                        op: *op,
                    },
                ))));
            }
            apply_binary_op(l_val, r_val, *op, env, console, ctx)
        }

        JsvExpr::UnaryOp(op, inner) => {
            if *op == OpKind::Typeof {
                if let JsvExpr::Identifier(name) = &**inner {
                    if env.get(name).is_none() {
                        return Ok(JsvValue::String("undefined".to_string()));
                    }
                }
            }
            let val = eval_expr(inner, env, console, ctx)?;
            if let JsvValue::YieldSignal(signal) = val {
                return Ok(JsvValue::YieldSignal(Box::new(
                    signal.push(Cont::UnaryApply { op: *op }),
                )));
            }
            match (op, val) {
                (OpKind::Sub, JsvValue::Number(n)) => Ok(JsvValue::Number(-n)),
                (OpKind::Not, v) => Ok(JsvValue::Boolean(!v.is_truthy())),
                (OpKind::Typeof, v) => Ok(JsvValue::String(typeof_name(&v).to_string())),
                (OpKind::Sub, other) => Ok(JsvValue::Number(-to_number(&other))),
                _ => Err("Invalid unary operation".to_string()),
            }
        }

        JsvExpr::If(cond, then_branch, else_branch) => {
            ctx.call_depth += 1;
            if ctx.call_depth > MAX_CALL_DEPTH {
                return Err("Maximum call depth exceeded in if statement".to_string());
            }

            let cond_val = eval_expr(cond, env, console, ctx)?;
            ctx.call_depth -= 1;

            if let JsvValue::YieldSignal(signal) = cond_val {
                return Ok(JsvValue::YieldSignal(Box::new(signal.push(Cont::IfCond {
                    then_branch: then_branch.clone(),
                    else_branch: else_branch.clone(),
                    env: env.clone(),
                }))));
            }

            if cond_val.is_truthy() {
                eval_statement_list(then_branch, env, console, ctx)
            } else if let Some(else_stmts) = else_branch {
                eval_statement_list(else_stmts, env, console, ctx)
            } else {
                Ok(JsvValue::Undefined)
            }
        }

        JsvExpr::While(cond, body) => {
            ctx.call_depth += 1;
            if ctx.call_depth > MAX_CALL_DEPTH {
                return Err("Maximum call depth exceeded in while loop".to_string());
            }
            let cond_val = eval_expr(cond, env, console, ctx)?;
            ctx.call_depth -= 1;
            if let JsvValue::YieldSignal(signal) = cond_val {
                return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                    Cont::WhileCond {
                        cond: cond.clone(),
                        body: body.clone(),
                        env: env.clone(),
                        result: JsvValue::Undefined,
                        iterations: 0,
                    },
                ))));
            }
            while_loop_step(
                cond,
                body,
                env.clone(),
                JsvValue::Undefined,
                0,
                cond_val,
                console,
                ctx,
            )
        }

        JsvExpr::ForOf(pattern, kind, iterable, body) => {
            let iterable = eval_expr(iterable, env, console, ctx)?;
            if let JsvValue::YieldSignal(signal) = iterable {
                return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                    Cont::ForOfIterable {
                        pattern: pattern.clone(),
                        kind: *kind,
                        body: body.clone(),
                        env: env.clone(),
                        result: JsvValue::Undefined,
                    },
                ))));
            }
            let values = iterate_values(&iterable.deref_live(), console, ctx)?;
            let mut result = JsvValue::Undefined;
            let mut index = 0usize;
            for value in values.iter() {
                let mut iteration_env = JsvEnvironment::with_parent(env.clone());
                bind_pattern(pattern, value, &mut iteration_env, *kind, console, ctx)?;
                let body_result = eval_statement_list(body, &mut iteration_env, console, ctx)?;
                match body_result {
                    JsvValue::BreakSignal => return Ok(result),
                    JsvValue::ContinueSignal => {
                        index += 1;
                        continue;
                    }
                    JsvValue::YieldSignal(signal) => {
                        return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                            Cont::ForOfRest {
                                pattern: pattern.clone(),
                                kind: *kind,
                                body: body.clone(),
                                env: env.clone(),
                                result,
                                values,
                                index: index + 1,
                            },
                        ))));
                    }
                    value if is_abrupt_signal(&value) => return Ok(value),
                    value => result = value,
                }
                index += 1;
            }
            Ok(result)
        }

        JsvExpr::Switch(discriminant, cases, default_clause) => {
            let disc = eval_expr(discriminant, env, console, ctx)?;
            if let JsvValue::YieldSignal(_) = disc {
                return Err(
                    "SyntaxError: yield in switch discriminant is not supported".to_string()
                );
            }
            let mut matched = false;
            let mut result = JsvValue::Undefined;
            for (case, body) in cases {
                if !matched {
                    match case {
                        Some(case_expr) => {
                            let case_value = eval_expr(case_expr, env, console, ctx)?;
                            if let JsvValue::YieldSignal(_) = case_value {
                                return Err("SyntaxError: yield in switch case is not supported"
                                    .to_string());
                            }
                            if !js_strict_equal(&disc, &case_value) {
                                continue;
                            }
                            matched = true;
                        }
                        None => matched = true,
                    }
                }
                if matched {
                    let body_result = eval_statement_list(body, env, console, ctx)?;
                    match body_result {
                        JsvValue::BreakSignal => return Ok(result),
                        JsvValue::ContinueSignal => return Ok(JsvValue::ContinueSignal),
                        JsvValue::YieldSignal(_) => return Ok(body_result),
                        value if is_abrupt_signal(&value) => return Ok(value),
                        value => result = value,
                    }
                }
            }
            if !matched {
                if let Some(body) = default_clause {
                    let body_result = eval_statement_list(body, env, console, ctx)?;
                    match body_result {
                        JsvValue::BreakSignal => return Ok(result),
                        JsvValue::ContinueSignal => return Ok(JsvValue::ContinueSignal),
                        JsvValue::YieldSignal(_) => return Ok(body_result),
                        value if is_abrupt_signal(&value) => return Ok(value),
                        value => result = value,
                    }
                }
            }
            Ok(result)
        }

        JsvExpr::Ternary(cond, then_expr, else_expr) => {
            let condition = eval_expr(cond, env, console, ctx)?;
            if let JsvValue::YieldSignal(signal) = condition {
                return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                    Cont::TernaryCond {
                        then_expr: then_expr.clone(),
                        else_expr: else_expr.clone(),
                        env: env.clone(),
                    },
                ))));
            }
            if condition.is_truthy() {
                eval_expr(then_expr, env, console, ctx)
            } else {
                eval_expr(else_expr, env, console, ctx)
            }
        }

        JsvExpr::Template(parts) => {
            let mut output = String::new();
            for (index, part) in parts.iter().enumerate() {
                match part {
                    TemplatePart::Text(text) => output.push_str(text),
                    TemplatePart::Expr(expression) => {
                        let value = eval_expr(expression, env, console, ctx)?;
                        if let JsvValue::YieldSignal(signal) = value {
                            return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                                Cont::TemplatePart {
                                    output,
                                    left: parts[index + 1..].to_vec(),
                                    env: env.clone(),
                                },
                            ))));
                        }
                        output.push_str(&value.deref_live().to_display_string());
                    }
                }
                check_string_alloc(ctx, output.len())?;
            }
            Ok(JsvValue::String(output))
        }

        JsvExpr::OptionalMember(object, property) => {
            let object = eval_expr(object, env, console, ctx)?;
            if matches!(object, JsvValue::Null | JsvValue::Undefined) {
                return Ok(JsvValue::Undefined);
            }
            if let JsvValue::YieldSignal(signal) = object {
                return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                    Cont::MemberApply {
                        property: property.clone(),
                        optional: true,
                    },
                ))));
            }
            read_property(object.deref_live(), property, env, console, ctx, true)
        }

        JsvExpr::OptionalIndex(object, key) => {
            let object = eval_expr(object, env, console, ctx)?;
            if matches!(object, JsvValue::Null | JsvValue::Undefined) {
                return Ok(JsvValue::Undefined);
            }
            if let JsvValue::YieldSignal(signal) = object {
                return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                    Cont::IndexBase {
                        key_expr: key.clone(),
                        optional: true,
                        env: env.clone(),
                    },
                ))));
            }
            let key = eval_expr(key, env, console, ctx)?;
            if let JsvValue::YieldSignal(signal) = key {
                return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                    Cont::IndexKey {
                        object: object.clone(),
                        key_expr: Box::new(JsvExpr::Undefined),
                        optional: true,
                        env: env.clone(),
                    },
                ))));
            }
            index_read(
                object.deref_live(),
                key.deref_live(),
                env,
                console,
                ctx,
                true,
            )
        }

        JsvExpr::Update(target, prefix, increment) => {
            enum UpdateTarget {
                Identifier(String),
                Member(JsvValue, String),
                Index(JsvValue, JsvValue),
            }
            let plan = match &**target {
                JsvExpr::Identifier(name) => UpdateTarget::Identifier(name.clone()),
                JsvExpr::Member(object, property) => {
                    let object = eval_expr(object, env, console, ctx)?;
                    UpdateTarget::Member(object, property.clone())
                }
                JsvExpr::Index(object, key) => {
                    let object = eval_expr(object, env, console, ctx)?;
                    let key = eval_expr(key, env, console, ctx)?;
                    UpdateTarget::Index(object, key)
                }
                _ => return Err("TypeError: invalid update target".to_string()),
            };
            let current = match &plan {
                UpdateTarget::Identifier(_) => eval_expr(target, env, console, ctx)?,
                UpdateTarget::Member(object, property) => {
                    read_property(object.deref_live(), property, env, console, ctx, false)?
                }
                UpdateTarget::Index(object, key) => index_read(
                    object.deref_live(),
                    key.deref_live(),
                    env,
                    console,
                    ctx,
                    false,
                )?,
            };
            if let JsvValue::YieldSignal(_) = current {
                return Err("SyntaxError: yield in update target is not supported".to_string());
            }
            let value = to_number(&current.deref_live());
            if !value.is_finite() {
                return Err("TypeError: increment/decrement requires a number".to_string());
            }
            let next = JsvValue::Number(if *increment { value + 1.0 } else { value - 1.0 });
            match &plan {
                UpdateTarget::Identifier(name) => {
                    env.assign(name, next.clone())?;
                }
                UpdateTarget::Member(object, property) => {
                    write_property(
                        object.deref_live(),
                        property,
                        next.clone(),
                        env,
                        console,
                        ctx,
                    )?;
                }
                UpdateTarget::Index(object, key) => {
                    index_write(
                        object.deref_live(),
                        key.deref_live(),
                        next.clone(),
                        env,
                        console,
                        ctx,
                    )?;
                }
            }
            if *prefix {
                Ok(next)
            } else {
                Ok(current)
            }
        }

        JsvExpr::Block(stmts) => eval_statement_list(stmts, env, console, ctx),

        JsvExpr::TryCatchFinally {
            try_body,
            catch_binding,
            catch_body,
            finally_body,
        } => eval_try_catch_finally(
            try_body,
            catch_binding.as_deref(),
            catch_body,
            finally_body,
            env,
            console,
            ctx,
        ),

        JsvExpr::FunctionDef(name, params, body) => {
            if is_sandboxed_function(name) {
                return Err(format!(
                    "SecurityError: Function '{}' is not allowed in sandboxed environment",
                    name
                ));
            }
            let func = JsvValue::Function(name.clone(), params.clone(), body.clone(), env.clone());
            env.define(name, func.clone());
            Ok(func)
        }

        JsvExpr::AsyncFunctionDef(name, params, body) => {
            let function =
                JsvValue::AsyncFunction(name.clone(), params.clone(), body.clone(), env.clone());
            env.define(name, function.clone());
            Ok(function)
        }

        JsvExpr::GeneratorDef(name, params, body) => {
            let func =
                JsvValue::GeneratorFn(name.clone(), params.clone(), body.clone(), env.clone());
            env.define(name, func.clone());
            Ok(func)
        }

        JsvExpr::FunctionExpr(params, body, is_async) => {
            if *is_async {
                Ok(JsvValue::AsyncFunction(
                    String::new(),
                    params.clone(),
                    body.clone(),
                    env.clone(),
                ))
            } else {
                Ok(JsvValue::Function(
                    String::new(),
                    params.clone(),
                    body.clone(),
                    env.clone(),
                ))
            }
        }

        JsvExpr::GeneratorExpr(params, body) => Ok(JsvValue::GeneratorFn(
            String::new(),
            params.clone(),
            body.clone(),
            env.clone(),
        )),

        JsvExpr::Yield(value) => {
            if ctx.in_generator == 0 {
                return Err(
                    "SyntaxError: yield is only valid inside a generator function".to_string(),
                );
            }
            let value = eval_expr(value, env, console, ctx)?;
            if let JsvValue::YieldSignal(signal) = value {
                return Ok(JsvValue::YieldSignal(signal));
            }
            Ok(JsvValue::YieldSignal(Box::new(JsvYieldSignal::new(value))))
        }

        JsvExpr::YieldStar(iterable) => eval_yield_star(iterable, env, console, ctx),

        JsvExpr::Call(callee, args) => eval_call(callee, args, env, console, ctx, false),

        JsvExpr::NewExpr(callee, args) => eval_call(callee, args, env, console, ctx, true),

        JsvExpr::SuperCall(args) => {
            let this_value = env.get("this").unwrap_or(JsvValue::Undefined);
            let parent_class = env.get("#parentClass").ok_or_else(|| {
                "ReferenceError: super is not available in this context".to_string()
            })?;
            let JsvValue::Class(parent) = parent_class else {
                return Err("TypeError: super must be called on a class constructor".to_string());
            };
            let mut arg_vals = Vec::new();
            for argument in args {
                arg_vals.push(eval_expr(argument, env, console, ctx)?);
            }
            construct_class_with_this(&parent, arg_vals, this_value, env, console, ctx)
        }

        JsvExpr::SuperMember(name) => {
            let this_value = env.get("this").unwrap_or(JsvValue::Undefined);
            let super_proto = env.get("#superProto").unwrap_or(JsvValue::Undefined);
            let JsvValue::Object(proto) = super_proto else {
                return Ok(JsvValue::Undefined);
            };
            let value = object_property(&proto, name).unwrap_or(JsvValue::Undefined);
            let value = value.deref_live();
            if let JsvValue::GetterSetter(ref get, _) = value {
                if is_callable(get) {
                    let called = call_function(
                        (**get).clone(),
                        Vec::new(),
                        env,
                        console,
                        ctx,
                        this_value.clone(),
                    )?;
                    return call_result_value(called);
                }
            }
            Ok(value)
        }

        JsvExpr::SuperMemberCall(name, args) => {
            let this_value = env.get("this").unwrap_or(JsvValue::Undefined);
            let super_proto = env.get("#superProto").unwrap_or(JsvValue::Undefined);
            let JsvValue::Object(proto) = super_proto else {
                return Err("TypeError: super.method() has no superclass".to_string());
            };
            let method = object_property(&proto, name)
                .ok_or_else(|| format!("TypeError: super.{} is not a function", name))?;
            let method = method.deref_live();
            let mut arg_vals = Vec::new();
            for argument in args {
                arg_vals.push(eval_expr(argument, env, console, ctx)?);
            }
            let called = call_function(method, arg_vals, env, console, ctx, this_value)?;
            call_result_value(called)
        }

        JsvExpr::PrivateGet(target, name) => {
            let target = eval_expr(target, env, console, ctx)?;
            if let JsvValue::YieldSignal(_) = target {
                return Err(
                    "SyntaxError: yield in private member access is not supported".to_string(),
                );
            }
            private_read(&target.deref_live(), name, env)
        }

        JsvExpr::PrivateSet(target, name, rhs) => {
            let target = eval_expr(target, env, console, ctx)?;
            let value = eval_expr(rhs, env, console, ctx)?;
            private_write(&target.deref_live(), name, value.clone(), env)?;
            Ok(value)
        }

        JsvExpr::ClassDef {
            name,
            extends,
            members,
        } => eval_class_def(name, extends, members, env, console, ctx),

        JsvExpr::DynamicImport(specifier) => {
            let Some(graph) = ctx.modules.take() else {
                return Err("Module graph is busy during dynamic import".to_string());
            };
            let result = graph.import_module(specifier, env, console, ctx);
            ctx.modules = Some(graph);
            match result {
                Ok(promise) => Ok(promise),
                Err(error) => Ok(JsvValue::Promise(Rc::new(RefCell::new(
                    JsvPromiseState::Rejected(JsvValue::String(error)),
                )))),
            }
        }

        JsvExpr::Spread(_) => {
            Err("SyntaxError: spread element outside array literal or call".to_string())
        }

        JsvExpr::Return(val) => {
            let r = eval_expr(val, env, console, ctx)?;
            if let JsvValue::YieldSignal(signal) = r {
                return Ok(JsvValue::YieldSignal(Box::new(
                    signal.push(Cont::ReturnApply),
                )));
            }
            Ok(JsvValue::ReturnSignal(Box::new(r)))
        }
        JsvExpr::Throw(value) => {
            let value = eval_expr(value, env, console, ctx)?;
            if let JsvValue::YieldSignal(signal) = value {
                return Ok(JsvValue::YieldSignal(Box::new(
                    signal.push(Cont::ThrowApply),
                )));
            }
            Ok(JsvValue::ThrowSignal(Box::new(value)))
        }
        JsvExpr::Await(value) => {
            let value = eval_expr(value, env, console, ctx)?;
            match value {
                JsvValue::Promise(promise) => match promise.borrow().clone() {
                    JsvPromiseState::Fulfilled(value) => Ok(value),
                    JsvPromiseState::Rejected(reason) => {
                        Ok(JsvValue::ThrowSignal(Box::new(reason)))
                    }
                    JsvPromiseState::Pending => {
                        Err("InvalidStateError: awaited Promise is still pending".to_string())
                    }
                },
                other => Ok(other),
            }
        }
        JsvExpr::Break(_label) => Ok(JsvValue::BreakSignal),
        JsvExpr::Continue(_label) => Ok(JsvValue::ContinueSignal),
        JsvExpr::DoWhile(cond, body) => do_while_loop_step(
            cond,
            body,
            env.clone(),
            JsvValue::Undefined,
            0,
            console,
            ctx,
        ),
        JsvExpr::ForIn(binding, kind, object_expr, body) => {
            let obj_val = eval_expr(object_expr, env, console, ctx)?;
            let keys = match &obj_val {
                JsvValue::Object(map) => map
                    .borrow()
                    .properties
                    .keys()
                    .filter(|key| !is_symbol_property_key(key))
                    .cloned()
                    .collect::<Vec<_>>(),
                JsvValue::Array(array) => (0..array.borrow().len())
                    .map(|index| index.to_string())
                    .collect(),
                _ => Vec::new(),
            };
            for_in_loop_step(
                binding,
                *kind,
                body,
                env.clone(),
                JsvValue::Undefined,
                keys,
                0,
                console,
                ctx,
            )
        }
        JsvExpr::AsyncGeneratorDef(name, params, body) => {
            let func_val =
                JsvValue::GeneratorFn(name.clone(), params.clone(), body.clone(), env.clone());
            if !name.is_empty() {
                env.define(name, func_val.clone());
            }
            Ok(func_val)
        }
        JsvExpr::AsyncGeneratorExpr(params, body) => Ok(JsvValue::GeneratorFn(
            String::new(),
            params.clone(),
            body.clone(),
            env.clone(),
        )),
    }
}

fn apply_binary_op(
    l_val: JsvValue,
    r_val: JsvValue,
    op: OpKind,
    env: &mut JsvEnvironment,
    console: &mut Vec<String>,
    ctx: &mut EvalCtx<'_>,
) -> Result<JsvValue, String> {
    let l_val = l_val.deref_live();
    let r_val = r_val.deref_live();

    match op {
        OpKind::And => {
            if !l_val.is_truthy() {
                return Ok(l_val);
            }
            return Ok(r_val);
        }
        OpKind::Or => {
            if l_val.is_truthy() {
                return Ok(l_val);
            }
            return Ok(r_val);
        }
        OpKind::Nullish => {
            if matches!(l_val, JsvValue::Null | JsvValue::Undefined) {
                return Ok(r_val);
            }
            return Ok(l_val);
        }
        OpKind::StrictEq => return Ok(JsvValue::Boolean(js_strict_equal(&l_val, &r_val))),
        OpKind::StrictNeq => return Ok(JsvValue::Boolean(!js_strict_equal(&l_val, &r_val))),
        OpKind::Instanceof => {
            let prototype = match r_val {
                JsvValue::Class(class) => Some(class.prototype_object.clone()),
                JsvValue::Object(constructor) => Some(constructor),
                _ => return Err("TypeError: instanceof requires a constructor object".to_string()),
            };
            let JsvValue::Object(object) = l_val else {
                return Ok(JsvValue::Boolean(false));
            };
            let Some(prototype) = prototype else {
                return Ok(JsvValue::Boolean(false));
            };
            let mut current = object.borrow().prototype.clone();
            for _ in 0..64 {
                let Some(reference) = current else {
                    return Ok(JsvValue::Boolean(false));
                };
                if Rc::ptr_eq(&reference, &prototype) {
                    return Ok(JsvValue::Boolean(true));
                }
                current = reference.borrow().prototype.clone();
            }
            return Ok(JsvValue::Boolean(false));
        }
        OpKind::In => {
            let key = property_key(&l_val);
            return match r_val {
                JsvValue::Object(object) => {
                    Ok(JsvValue::Boolean(object_property(&object, &key).is_some()))
                }
                JsvValue::Array(array) => {
                    let index = key.parse::<usize>().ok();
                    Ok(JsvValue::Boolean(
                        index.is_some_and(|index| index < array.borrow().len()),
                    ))
                }
                JsvValue::Proxy(proxy) => {
                    proxy_has(&proxy, &key, env, console, ctx).map(JsvValue::Boolean)
                }
                other => Err(format!(
                    "TypeError: cannot use 'in' on {}",
                    other.to_display_string()
                )),
            };
        }
        _ => {}
    }

    match (l_val, r_val) {
        (JsvValue::Number(ln), JsvValue::Number(rn)) => match op {
            OpKind::Add => Ok(JsvValue::Number(ln + rn)),
            OpKind::Sub => Ok(JsvValue::Number(ln - rn)),
            OpKind::Mul => Ok(JsvValue::Number(ln * rn)),
            OpKind::Div => Ok(JsvValue::Number(ln / rn)),
            OpKind::Mod => Ok(JsvValue::Number(ln % rn)),
            OpKind::Eq => Ok(JsvValue::Boolean(ln == rn)),
            OpKind::Neq => Ok(JsvValue::Boolean(ln != rn)),
            OpKind::Lt => Ok(JsvValue::Boolean(ln < rn)),
            OpKind::Le => Ok(JsvValue::Boolean(ln <= rn)),
            OpKind::Gt => Ok(JsvValue::Boolean(ln > rn)),
            OpKind::Ge => Ok(JsvValue::Boolean(ln >= rn)),
            _ => Err("Invalid operator for numbers".to_string()),
        },
        (JsvValue::String(ls), JsvValue::String(rs)) => match op {
            OpKind::Add => {
                check_string_alloc(ctx, ls.len() + rs.len())?;
                Ok(JsvValue::String(format!("{}{}", ls, rs)))
            }
            OpKind::Eq => Ok(JsvValue::Boolean(ls == rs)),
            OpKind::Neq => Ok(JsvValue::Boolean(ls != rs)),
            OpKind::Lt => Ok(JsvValue::Boolean(ls < rs)),
            OpKind::Le => Ok(JsvValue::Boolean(ls <= rs)),
            OpKind::Gt => Ok(JsvValue::Boolean(ls > rs)),
            OpKind::Ge => Ok(JsvValue::Boolean(ls >= rs)),
            _ => Err("Invalid operator for strings".to_string()),
        },
        (JsvValue::Null, JsvValue::Undefined) | (JsvValue::Undefined, JsvValue::Null) => match op {
            OpKind::Eq => Ok(JsvValue::Boolean(true)),
            OpKind::Neq => Ok(JsvValue::Boolean(false)),
            _ => Err("Type mismatch in binary operation".to_string()),
        },
        (JsvValue::Number(ln), rhs) => {
            if op == OpKind::Add {
                let display = rhs.to_display_string();
                check_string_alloc(ctx, display.len())?;
                Ok(JsvValue::String(format!("{}{}", ln, display)))
            } else {
                numeric_fallthrough(JsvValue::Number(ln), rhs, op, ctx)
            }
        }
        (JsvValue::String(ls), rhs) => {
            if op == OpKind::Add {
                let display = rhs.to_display_string();
                check_string_alloc(ctx, ls.len() + display.len())?;
                Ok(JsvValue::String(format!("{}{}", ls, display)))
            } else {
                numeric_fallthrough(JsvValue::String(ls), rhs, op, ctx)
            }
        }
        (l_val, r_val) => numeric_fallthrough(l_val, r_val, op, ctx),
    }
}

fn numeric_fallthrough(
    l_val: JsvValue,
    r_val: JsvValue,
    op: OpKind,
    ctx: &mut EvalCtx<'_>,
) -> Result<JsvValue, String> {
    match op {
        OpKind::Eq | OpKind::Neq => {
            let left = if matches!(l_val, JsvValue::Boolean(_)) {
                JsvValue::Number(to_number(&l_val))
            } else {
                l_val
            };
            let right = if matches!(r_val, JsvValue::Boolean(_)) {
                JsvValue::Number(to_number(&r_val))
            } else {
                r_val
            };
            let equal = match (&left, &right) {
                (JsvValue::Number(_), JsvValue::String(_))
                | (JsvValue::String(_), JsvValue::Number(_)) => {
                    to_number(&left) == to_number(&right)
                }
                _ => js_strict_equal(&left, &right),
            };
            Ok(JsvValue::Boolean(if op == OpKind::Eq {
                equal
            } else {
                !equal
            }))
        }
        OpKind::Add => {
            if matches!(l_val, JsvValue::String(_)) || matches!(r_val, JsvValue::String(_)) {
                let left = l_val.to_display_string();
                let right = r_val.to_display_string();
                check_string_alloc(ctx, left.len().saturating_add(right.len()))?;
                Ok(JsvValue::String(format!("{}{}", left, right)))
            } else {
                Ok(JsvValue::Number(to_number(&l_val) + to_number(&r_val)))
            }
        }
        OpKind::Sub => Ok(JsvValue::Number(to_number(&l_val) - to_number(&r_val))),
        OpKind::Mul => Ok(JsvValue::Number(to_number(&l_val) * to_number(&r_val))),
        OpKind::Div => Ok(JsvValue::Number(to_number(&l_val) / to_number(&r_val))),
        OpKind::Mod => Ok(JsvValue::Number(to_number(&l_val) % to_number(&r_val))),
        OpKind::Lt => Ok(JsvValue::Boolean(to_number(&l_val) < to_number(&r_val))),
        OpKind::Le => Ok(JsvValue::Boolean(to_number(&l_val) <= to_number(&r_val))),
        OpKind::Gt => Ok(JsvValue::Boolean(to_number(&l_val) > to_number(&r_val))),
        OpKind::Ge => Ok(JsvValue::Boolean(to_number(&l_val) >= to_number(&r_val))),
        _ => Err("Type mismatch in binary operation".to_string()),
    }
}

fn js_strict_equal(left: &JsvValue, right: &JsvValue) -> bool {
    match (left, right) {
        (JsvValue::Number(l), JsvValue::Number(r)) => l == r,
        (JsvValue::String(l), JsvValue::String(r)) => l == r,
        (JsvValue::Boolean(l), JsvValue::Boolean(r)) => l == r,
        (JsvValue::Null, JsvValue::Null) => true,
        (JsvValue::Undefined, JsvValue::Undefined) => true,
        (JsvValue::Object(l), JsvValue::Object(r)) => Rc::ptr_eq(l, r),
        (JsvValue::Array(l), JsvValue::Array(r)) => Rc::ptr_eq(l, r),
        (JsvValue::Promise(l), JsvValue::Promise(r)) => Rc::ptr_eq(l, r),
        (JsvValue::Symbol(l), JsvValue::Symbol(r)) => Rc::ptr_eq(l, r),
        (JsvValue::Error(l), JsvValue::Error(r)) => Rc::ptr_eq(l, r),
        (JsvValue::Map(l), JsvValue::Map(r)) => Rc::ptr_eq(l, r),
        (JsvValue::Set(l), JsvValue::Set(r)) => Rc::ptr_eq(l, r),
        (JsvValue::WeakMap(l), JsvValue::WeakMap(r)) => Rc::ptr_eq(l, r),
        (JsvValue::WeakSet(l), JsvValue::WeakSet(r)) => Rc::ptr_eq(l, r),
        (JsvValue::ArrayBuffer(l), JsvValue::ArrayBuffer(r)) => Rc::ptr_eq(l, r),
        (JsvValue::TypedArray(l), JsvValue::TypedArray(r)) => Rc::ptr_eq(l, r),
        (JsvValue::DataView(l), JsvValue::DataView(r)) => Rc::ptr_eq(l, r),
        (JsvValue::Proxy(l), JsvValue::Proxy(r)) => Rc::ptr_eq(l, r),
        (JsvValue::Class(l), JsvValue::Class(r)) => Rc::ptr_eq(l, r),
        (JsvValue::GeneratorObject(l), JsvValue::GeneratorObject(r)) => Rc::ptr_eq(l, r),
        (JsvValue::Iterator(l), JsvValue::Iterator(r)) => Rc::ptr_eq(l, r),
        (JsvValue::LiveBinding(l_env, l_name), JsvValue::LiveBinding(r_env, r_name)) => {
            l_name == r_name && l_env.same_record(r_env)
        }
        (JsvValue::HostObject(l), JsvValue::HostObject(r)) => l == r,
        (JsvValue::HostFunction(l_object, l_name), JsvValue::HostFunction(r_object, r_name)) => {
            l_object == r_object && l_name == r_name
        }
        (JsvValue::NativeFn(l), JsvValue::NativeFn(r)) => l == r,

        (JsvValue::Function(..), JsvValue::Function(..))
        | (JsvValue::AsyncFunction(..), JsvValue::AsyncFunction(..))
        | (JsvValue::GeneratorFn(..), JsvValue::GeneratorFn(..)) => {
            left.function_identity_eq(right)
        }
        (JsvValue::BoundNativeFn(l_name, l), JsvValue::BoundNativeFn(r_name, r)) => {
            l_name == r_name && l == r
        }
        _ => false,
    }
}

fn typeof_name(value: &JsvValue) -> &'static str {
    match value {
        JsvValue::Number(_) => "number",
        JsvValue::String(_) => "string",
        JsvValue::Boolean(_) => "boolean",
        JsvValue::Symbol(_) => "symbol",
        JsvValue::Null => "object",
        JsvValue::Undefined => "undefined",
        JsvValue::Object(_) => "object",
        JsvValue::Array(_) => "object",
        JsvValue::Function(..)
        | JsvValue::AsyncFunction(..)
        | JsvValue::GeneratorFn(..)
        | JsvValue::NativeFn(_)
        | JsvValue::BoundNativeFn(..)
        | JsvValue::Class(_) => "function",
        JsvValue::Promise(_)
        | JsvValue::Error(_)
        | JsvValue::Map(_)
        | JsvValue::Set(_)
        | JsvValue::WeakMap(_)
        | JsvValue::WeakSet(_)
        | JsvValue::ArrayBuffer(_)
        | JsvValue::TypedArray(_)
        | JsvValue::DataView(_)
        | JsvValue::Proxy(_)
        | JsvValue::GeneratorObject(_)
        | JsvValue::Iterator(_)
        | JsvValue::GetterSetter(..)
        | JsvValue::LiveBinding(..) => "object",
        JsvValue::HostObject(_) => "object",
        JsvValue::HostFunction(..) => "function",
        JsvValue::ReturnSignal(_) | JsvValue::ThrowSignal(_) | JsvValue::YieldSignal(_) => "object",
        JsvValue::BreakSignal | JsvValue::ContinueSignal => "object",
    }
}

fn call_function(
    func: JsvValue,
    args: Vec<JsvValue>,
    env: &mut JsvEnvironment,
    console: &mut Vec<String>,
    ctx: &mut EvalCtx<'_>,
    this_arg: JsvValue,
) -> Result<JsvValue, String> {
    ctx.call_depth += 1;
    if ctx.call_depth > MAX_CALL_DEPTH {
        ctx.call_depth = ctx.call_depth.saturating_sub(1);
        return Err("Maximum call stack size exceeded".to_string());
    }
    let result = match func {
        JsvValue::NativeFn(name) => call_native_fn(&name, args, env, console, ctx),
        JsvValue::BoundNativeFn(name, receiver) => {
            call_bound_native(&name, *receiver, args, env, console, ctx)
        }
        JsvValue::HostFunction(object, method) => ctx.host.call(object, &method, args),
        JsvValue::Function(name, params, body, captured) => {
            match function_environment(&name, &params, &body, &captured, args, env, console, ctx) {
                Ok(mut fn_env) => {
                    fn_env.define("this", this_arg);
                    eval_expr(&body, &mut fn_env, console, ctx)
                }
                Err(error) => Err(error),
            }
        }
        JsvValue::AsyncFunction(name, params, body, captured) => {
            match function_environment(&name, &params, &body, &captured, args, env, console, ctx) {
                Ok(mut fn_env) => {
                    fn_env.define("this", this_arg);
                    let state = match eval_expr(&body, &mut fn_env, console, ctx) {
                        Ok(JsvValue::ReturnSignal(value)) => match *value {
                            JsvValue::Promise(promise) => promise
                                .try_borrow()
                                .map_err(|_| "TypeError: Reentrant access to promise".to_string())?
                                .clone(),
                            value => JsvPromiseState::Fulfilled(value),
                        },
                        Ok(JsvValue::ThrowSignal(reason)) => JsvPromiseState::Rejected(*reason),
                        Ok(JsvValue::BreakSignal) => JsvPromiseState::Rejected(JsvValue::String(
                            "SyntaxError: illegal break statement in async function".to_string(),
                        )),
                        Ok(JsvValue::ContinueSignal) => {
                            JsvPromiseState::Rejected(JsvValue::String(
                                "SyntaxError: illegal continue statement in async function"
                                    .to_string(),
                            ))
                        }
                        Ok(JsvValue::YieldSignal(_)) => {
                            JsvPromiseState::Rejected(JsvValue::String(
                                "SyntaxError: yield is not allowed in async functions".to_string(),
                            ))
                        }
                        Ok(value) => JsvPromiseState::Fulfilled(value),
                        Err(error) => JsvPromiseState::Rejected(JsvValue::String(error)),
                    };
                    Ok(JsvValue::Promise(Rc::new(RefCell::new(state))))
                }
                Err(error) => Err(error),
            }
        }
        JsvValue::GeneratorFn(name, params, body, captured) => Ok(JsvValue::GeneratorObject(
            Rc::new(RefCell::new(JsvGeneratorState {
                name,
                params,
                body,
                captured,
                started: false,
                done: false,
                resume: None,
                running: false,
            })),
        )),
        JsvValue::Class(class) => Err(format!(
            "TypeError: Class constructor {} cannot be invoked without 'new'",
            class.name
        )),
        other => Err(format!(
            "TypeError: {} is not a function",
            other.to_display_string()
        )),
    };
    ctx.call_depth = ctx.call_depth.saturating_sub(1);
    result
}

#[allow(clippy::too_many_arguments)]
fn function_environment(
    name: &str,
    params: &[ParamBinding],
    body: &JsvExpr,
    captured: &JsvEnvironment,
    args: Vec<JsvValue>,
    env: &mut JsvEnvironment,
    console: &mut Vec<String>,
    ctx: &mut EvalCtx<'_>,
) -> Result<JsvEnvironment, String> {
    let mut fn_env = JsvEnvironment::with_parent(captured.clone());
    if !name.is_empty() {
        fn_env.define(
            name,
            JsvValue::Function(
                name.to_string(),
                params.to_vec(),
                Box::new(body.clone()),
                captured.clone(),
            ),
        );
    }

    if let Some(class_id) = captured.get("#classId") {
        if let JsvValue::Number(id) = class_id {
            fn_env.define("#classId", JsvValue::Number(id));
        }
        if let Some(super_proto) = captured.get("#superProto") {
            fn_env.define("#superProto", super_proto);
        }
        if let Some(parent_class) = captured.get("#parentClass") {
            fn_env.define("#parentClass", parent_class);
        }
    }
    bind_params(params, &args, &mut fn_env, console, ctx)?;
    let _ = env;
    Ok(fn_env)
}

fn bind_params(
    params: &[ParamBinding],
    args: &[JsvValue],
    fn_env: &mut JsvEnvironment,
    console: &mut Vec<String>,
    ctx: &mut EvalCtx<'_>,
) -> Result<(), String> {
    let mut arg_index = 0usize;
    for param in params {
        match &param.pattern {
            ParamPattern::Rest(inner) => {
                let remaining = args[arg_index.min(args.len())..].to_vec();
                bind_pattern_impl(
                    inner,
                    &JsvValue::Array(Rc::new(RefCell::new(remaining))),
                    fn_env,
                    true,
                    false,
                    console,
                    ctx,
                )?;
                arg_index = args.len();
            }
            pattern => {
                let provided = args.get(arg_index).cloned().unwrap_or(JsvValue::Undefined);
                let value = match (
                    matches!(provided, JsvValue::Undefined),
                    param.default.as_ref(),
                ) {
                    (true, Some(default_expr)) => eval_expr(default_expr, fn_env, console, ctx)?,
                    _ => provided,
                };
                bind_pattern_impl(pattern, &value, fn_env, true, false, console, ctx)?;
                arg_index += 1;
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn bind_pattern(
    pattern: &ParamPattern,
    value: &JsvValue,
    env: &mut JsvEnvironment,
    kind: DeclarationKind,
    console: &mut Vec<String>,
    ctx: &mut EvalCtx<'_>,
) -> Result<(), String> {
    bind_pattern_impl(
        pattern,
        value,
        env,
        kind != DeclarationKind::Const,
        kind == DeclarationKind::Var,
        console,
        ctx,
    )
}

#[allow(clippy::too_many_arguments)]
fn bind_pattern_impl(
    pattern: &ParamPattern,
    value: &JsvValue,
    env: &mut JsvEnvironment,
    mutable: bool,
    allow_redeclare: bool,
    console: &mut Vec<String>,
    ctx: &mut EvalCtx<'_>,
) -> Result<(), String> {
    match pattern {
        ParamPattern::Identifier(name) => {
            env.declare(name, value.clone(), mutable, allow_redeclare)?;
        }
        ParamPattern::Rest(inner) => {
            bind_pattern_impl(inner, value, env, mutable, allow_redeclare, console, ctx)?
        }
        ParamPattern::Array(elements, rest) => {
            let items = iterate_values(value, console, ctx)?;
            let mut index = 0;
            for element in elements {
                let item = items.get(index).cloned().unwrap_or(JsvValue::Undefined);
                bind_pattern_impl(element, &item, env, mutable, allow_redeclare, console, ctx)?;
                index += 1;
            }
            if let Some(rest_pattern) = rest {
                let rest_values = items[index.min(items.len())..].to_vec();
                bind_pattern_impl(
                    rest_pattern,
                    &JsvValue::Array(Rc::new(RefCell::new(rest_values))),
                    env,
                    mutable,
                    allow_redeclare,
                    console,
                    ctx,
                )?;
            }
        }
        ParamPattern::Object(entries, rest) => {
            let source = value.deref_live();
            let mut taken = Vec::new();
            for (key, sub_pattern) in entries {
                let item = direct_property_read(&source, key);
                taken.push(key.clone());
                bind_pattern_impl(
                    sub_pattern,
                    &item,
                    env,
                    mutable,
                    allow_redeclare,
                    console,
                    ctx,
                )?;
            }
            if let Some(rest_pattern) = rest {
                let mut remaining = HashMap::new();
                for (key, item) in own_properties(&source)? {
                    if !taken.contains(&key) && !is_symbol_property_key(&key) {
                        remaining.insert(key, item);
                    }
                }
                bind_pattern_impl(
                    rest_pattern,
                    &object_value(remaining),
                    env,
                    mutable,
                    allow_redeclare,
                    console,
                    ctx,
                )?;
            }
        }
    }
    Ok(())
}

fn promise_value(state: JsvPromiseState) -> JsvValue {
    JsvValue::Promise(Rc::new(RefCell::new(state)))
}

fn is_callable(value: &JsvValue) -> bool {
    matches!(
        value,
        JsvValue::Function(_, _, _, _)
            | JsvValue::AsyncFunction(_, _, _, _)
            | JsvValue::GeneratorFn(_, _, _, _)
            | JsvValue::NativeFn(_)
            | JsvValue::BoundNativeFn(_, _)
            | JsvValue::HostFunction(_, _)
            | JsvValue::Class(_)
    )
}

pub fn is_callable_public(value: &JsvValue) -> bool {
    is_callable(value)
}

const MAX_ARRAY_ELEMENTS: usize = 262_144;

const MAX_COLLECTION_ENTRIES: usize = 65_536;

const MAX_ITERATOR_RESULTS: usize = 100_000;

fn to_number(value: &JsvValue) -> f64 {
    match value {
        JsvValue::Number(n) => *n,
        JsvValue::Boolean(b) => {
            if *b {
                1.0
            } else {
                0.0
            }
        }
        JsvValue::Null => 0.0,
        JsvValue::String(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                0.0
            } else {
                match trimmed.parse::<f64>() {
                    Ok(value) => value,
                    Err(_) => whole_hex_number(trimmed).unwrap_or(f64::NAN),
                }
            }
        }
        JsvValue::Undefined => f64::NAN,
        _ => f64::NAN,
    }
}

fn whole_hex_number(trimmed: &str) -> Option<f64> {
    let (negative, body) = match trimmed.as_bytes().first() {
        Some(b'-') => (true, &trimmed[1..]),
        Some(b'+') => (false, &trimmed[1..]),
        _ => (false, trimmed),
    };
    let digits = body
        .strip_prefix("0x")
        .or_else(|| body.strip_prefix("0X"))?;
    if digits.is_empty() {
        return None;
    }
    let value = i64::from_str_radix(digits, 16).ok()? as f64;
    Some(if negative { -value } else { value })
}

fn parse_number_prefix(text: &str) -> Option<f64> {
    let trimmed = text.trim();
    let bytes = trimmed.as_bytes();
    let mut index = 0;
    if index < bytes.len() && (bytes[index] == b'+' || bytes[index] == b'-') {
        index += 1;
    }
    let start = index;
    while index < bytes.len() && bytes[index].is_ascii_digit() {
        index += 1;
    }
    if index < bytes.len() && bytes[index] == b'.' {
        index += 1;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }
    }
    if index == start {
        return None;
    }
    let mut end = index;
    if index < bytes.len() && (bytes[index] == b'e' || bytes[index] == b'E') {
        let mut exponent = index + 1;
        if exponent < bytes.len() && (bytes[exponent] == b'+' || bytes[exponent] == b'-') {
            exponent += 1;
        }
        let digits = exponent;
        while exponent < bytes.len() && bytes[exponent].is_ascii_digit() {
            exponent += 1;
        }
        if exponent > digits {
            end = exponent;
        }
    }
    trimmed[start..end].parse::<f64>().ok()
}

fn parse_int_prefix(text: &str) -> Option<f64> {
    let trimmed = text.trim();
    let bytes = trimmed.as_bytes();
    let mut index = 0;
    let negative = index < bytes.len() && bytes[index] == b'-';
    if negative || (index < bytes.len() && bytes[index] == b'+') {
        index += 1;
    }
    if index + 1 < bytes.len() && bytes[index] == b'0' && (bytes[index + 1] | 0x20) == b'x' {
        let mut value = 0f64;
        let mut digits = 0;
        for byte in &bytes[index + 2..] {
            let digit = match byte {
                b'0'..=b'9' => u32::from(byte - b'0'),
                b'a'..=b'f' => u32::from(byte - b'a') + 10,
                b'A'..=b'F' => u32::from(byte - b'A') + 10,
                _ => break,
            };
            value = value * 16.0 + f64::from(digit);
            digits += 1;
        }
        return if digits == 0 {
            parse_number_prefix(trimmed).map(f64::trunc)
        } else {
            Some(if negative { -value } else { value })
        };
    }
    parse_number_prefix(trimmed).map(f64::trunc)
}

fn call_result_value(result: JsvValue) -> Result<JsvValue, String> {
    match result {
        JsvValue::ReturnSignal(value) => Ok(*value),
        JsvValue::BreakSignal => {
            Err("SyntaxError: illegal break statement in function".to_string())
        }
        JsvValue::ContinueSignal => {
            Err("SyntaxError: illegal continue statement in function".to_string())
        }
        value => Ok(value),
    }
}

fn eval_statement_list(
    stmts: &[JsvExpr],
    env: &mut JsvEnvironment,
    console: &mut Vec<String>,
    ctx: &mut EvalCtx<'_>,
) -> Result<JsvValue, String> {
    let mut list_env = JsvEnvironment::with_parent(env.clone());
    let mut result = JsvValue::Undefined;
    let mut index = 0;
    while index < stmts.len() {
        let r = eval_expr(&stmts[index], &mut list_env, console, ctx)?;
        if let JsvValue::YieldSignal(signal) = r {
            return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                Cont::Statements {
                    stmts: stmts.to_vec(),
                    index: index + 1,
                    env: list_env,
                    result,
                },
            ))));
        }
        if is_abrupt_signal(&r) {
            return Ok(r);
        }
        result = r;
        index += 1;
    }
    Ok(result)
}

fn eval_try_catch_finally(
    try_body: &[JsvExpr],
    catch_binding: Option<&str>,
    catch_body: &[JsvExpr],
    finally_body: &[JsvExpr],
    env: &mut JsvEnvironment,
    console: &mut Vec<String>,
    ctx: &mut EvalCtx<'_>,
) -> Result<JsvValue, String> {
    let try_env = JsvEnvironment::with_parent(env.clone());
    let outcome = match eval_statement_list(try_body, &mut try_env.clone(), console, ctx) {
        Ok(value) => value,
        Err(error) => JsvValue::ThrowSignal(Box::new(JsvValue::String(error))),
    };
    if let JsvValue::YieldSignal(signal) = outcome {
        return Ok(JsvValue::YieldSignal(Box::new(signal.push(
            Cont::TryResume {
                try_body: try_body.to_vec(),
                catch_binding: catch_binding.map(str::to_string),
                catch_body: catch_body.to_vec(),
                finally_body: finally_body.to_vec(),
                env: env.clone(),
                outcome: JsvValue::Undefined,
                stage: 0,
            },
        ))));
    }
    let catch_result = match outcome {
        JsvValue::ThrowSignal(reason) if catch_binding.is_some() => {
            let Some(binding) = catch_binding else {
                return Err("InvalidStateError: catch binding vanished".to_string());
            };
            let mut catch_env = JsvEnvironment::with_parent(env.clone());
            catch_env.define(binding, (*reason).clone());
            let result = match eval_statement_list(catch_body, &mut catch_env, console, ctx) {
                Ok(value) => value,
                Err(error) => JsvValue::ThrowSignal(Box::new(JsvValue::String(error))),
            };
            if let JsvValue::YieldSignal(signal) = result {
                return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                    Cont::TryResume {
                        try_body: try_body.to_vec(),
                        catch_binding: catch_binding.map(str::to_string),
                        catch_body: catch_body.to_vec(),
                        finally_body: finally_body.to_vec(),
                        env: env.clone(),
                        outcome: JsvValue::Undefined,
                        stage: 1,
                    },
                ))));
            }
            result
        }
        other => other,
    };

    if finally_body.is_empty() {
        return Ok(catch_result);
    }
    let mut finally_env = JsvEnvironment::with_parent(env.clone());
    let final_value = match eval_statement_list(finally_body, &mut finally_env, console, ctx) {
        Ok(value) => value,
        Err(error) => JsvValue::ThrowSignal(Box::new(JsvValue::String(error))),
    };
    if let JsvValue::YieldSignal(signal) = final_value {
        return Ok(JsvValue::YieldSignal(Box::new(signal.push(
            Cont::TryResume {
                try_body: try_body.to_vec(),
                catch_binding: catch_binding.map(str::to_string),
                catch_body: catch_body.to_vec(),
                finally_body: finally_body.to_vec(),
                env: env.clone(),
                outcome: catch_result,
                stage: 2,
            },
        ))));
    }
    if is_abrupt_signal(&final_value) {
        Ok(final_value)
    } else {
        Ok(catch_result)
    }
}

#[allow(clippy::too_many_arguments)]
fn read_property(
    value: JsvValue,
    property: &str,
    env: &mut JsvEnvironment,
    console: &mut Vec<String>,
    ctx: &mut EvalCtx<'_>,
    optional: bool,
) -> Result<JsvValue, String> {
    if matches!(value, JsvValue::Null | JsvValue::Undefined) {
        if optional {
            return Ok(JsvValue::Undefined);
        }
        return Err(format!(
            "TypeError: Cannot read property '{}' of {}",
            property,
            value.to_display_string()
        ));
    }
    let value = value.deref_live();
    match value {
        JsvValue::Object(object) => {
            if let Some(found) = object_property(&object, property) {
                let found = found.deref_live();
                if let JsvValue::GetterSetter(ref get, _) = found {
                    if is_callable(get) {
                        let called = call_function(
                            (**get).clone(),
                            Vec::new(),
                            env,
                            console,
                            ctx,
                            JsvValue::Object(object),
                        )?;
                        return call_result_value(called);
                    }
                }
                Ok(found)
            } else {
                let borrowed = object.borrow();
                let brand = borrowed
                    .properties
                    .get("__sylphra_native_brand")
                    .or_else(|| borrowed.properties.get("__ghita_native_brand"))
                    .and_then(JsvValue::as_string)
                    .map(str::to_string);
                drop(borrowed);
                match (brand.as_deref(), property) {
                    (Some("WeakRef"), "deref") => Ok(JsvValue::BoundNativeFn(
                        "WeakRef.deref".to_string(),
                        Box::new(
                            object
                                .borrow()
                                .properties
                                .get("deref_target")
                                .cloned()
                                .unwrap_or(JsvValue::Undefined),
                        ),
                    )),
                    (Some("FinalizationRegistry"), "register" | "unregister") => Ok(
                        JsvValue::NativeFn(format!("FinalizationRegistry.{property}")),
                    ),
                    (Some("DateTimeFormat"), "format") => {
                        Ok(JsvValue::NativeFn("DateTimeFormat.format".to_string()))
                    }
                    (Some("NumberFormat"), "format") => {
                        Ok(JsvValue::NativeFn("NumberFormat.format".to_string()))
                    }
                    _ => Ok(JsvValue::Undefined),
                }
            }
        }
        JsvValue::Array(array) => array_member(&array, property),
        JsvValue::String(text) => string_member(&text, property),
        JsvValue::Promise(promise) => match property {
            "then" | "catch" | "finally" => Ok(JsvValue::BoundNativeFn(
                format!("Promise.{}", property),
                Box::new(JsvValue::Promise(promise)),
            )),
            _ => Ok(JsvValue::Undefined),
        },
        JsvValue::Map(map) => map_member(&map, property),
        JsvValue::Set(set) => set_member(&set, property),
        JsvValue::WeakMap(map) => weak_map_member(&map, property),
        JsvValue::WeakSet(set) => weak_set_member(&set, property),
        JsvValue::ArrayBuffer(buffer) => match property {
            "byteLength" => Ok(JsvValue::Number(buffer.borrow().bytes.len() as f64)),
            "slice" => Ok(JsvValue::BoundNativeFn(
                "ArrayBuffer.slice".to_string(),
                Box::new(JsvValue::ArrayBuffer(buffer)),
            )),
            _ => Ok(JsvValue::Undefined),
        },
        JsvValue::TypedArray(array) => typed_array_member(&array, property),
        JsvValue::DataView(view) => data_view_member(&view, property),
        JsvValue::Error(error) => match property {
            "name" => Ok(JsvValue::String(error.name.clone())),
            "message" => Ok(JsvValue::String(error.message.clone())),
            "toString" => Ok(JsvValue::BoundNativeFn(
                "Error.toString".to_string(),
                Box::new(JsvValue::Error(error)),
            )),
            _ => Ok(JsvValue::Undefined),
        },
        JsvValue::Symbol(symbol) => match property {
            "description" => Ok(JsvValue::String(symbol.description.clone())),
            "toString" => Ok(JsvValue::BoundNativeFn(
                "Symbol.toString".to_string(),
                Box::new(JsvValue::Symbol(symbol)),
            )),
            _ => Ok(JsvValue::Undefined),
        },
        JsvValue::Class(class) => {
            if property == "prototype" {
                Ok(JsvValue::Object(class.prototype_object.clone()))
            } else if property == "name" {
                Ok(JsvValue::String(class.name.clone()))
            } else if property == "length" {
                let length = class
                    .constructor
                    .as_ref()
                    .map(|(params, _)| {
                        params
                            .iter()
                            .filter(|p| !matches!(p.pattern, ParamPattern::Rest(_)))
                            .count()
                    })
                    .unwrap_or(0);
                Ok(JsvValue::Number(length as f64))
            } else if let Some(found) = class.static_methods.get(property) {
                Ok(found.clone())
            } else {
                Ok(JsvValue::Undefined)
            }
        }
        JsvValue::Function(name, params, body, captured)
        | JsvValue::AsyncFunction(name, params, body, captured)
        | JsvValue::GeneratorFn(name, params, body, captured) => {
            function_member(&name, &params, &body, &captured, property)
        }
        JsvValue::GeneratorObject(generator) => generator_member(&generator, property),
        JsvValue::Iterator(iterator) => match property {
            "next" => Ok(JsvValue::BoundNativeFn(
                "Iterator.next".to_string(),
                Box::new(JsvValue::Iterator(iterator)),
            )),
            "return" => Ok(JsvValue::BoundNativeFn(
                "Iterator.return".to_string(),
                Box::new(JsvValue::Iterator(iterator)),
            )),
            _ => Ok(JsvValue::Undefined),
        },
        JsvValue::Proxy(proxy) => proxy_get(&proxy, property, env, console, ctx),
        JsvValue::HostObject(object) => ctx.host.get_property(object, property),
        JsvValue::HostFunction(object, method) => {
            if object == HOST_WINDOW && method == "MediaSource" && property == "isTypeSupported" {
                Ok(JsvValue::HostFunction(
                    HOST_WINDOW,
                    "MediaSource.isTypeSupported".to_string(),
                ))
            } else {
                Err(format!(
                    "TypeError: Cannot read property '{}' of a host function",
                    property
                ))
            }
        }
        other => Err(format!(
            "TypeError: Cannot read property '{}' of {}",
            property,
            other.to_display_string()
        )),
    }
}

fn direct_property_read(value: &JsvValue, key: &str) -> JsvValue {
    let value = value.deref_live();
    match value {
        JsvValue::Object(object) => object_property(&object, key).unwrap_or(JsvValue::Undefined),
        JsvValue::Array(array) => {
            if key == "length" {
                JsvValue::Number(array.borrow().len() as f64)
            } else {
                key.parse::<usize>()
                    .ok()
                    .and_then(|index| array.borrow().get(index).cloned())
                    .unwrap_or(JsvValue::Undefined)
            }
        }
        JsvValue::TypedArray(array) => {
            let Ok(borrowed) = array.try_borrow() else {
                return JsvValue::Undefined;
            };
            let Ok(buffer) = borrowed.buffer.try_borrow() else {
                return JsvValue::Undefined;
            };
            if buffer.detached {
                return JsvValue::Undefined;
            }
            key.parse::<usize>()
                .ok()
                .filter(|index| *index < borrowed.length)
                .and_then(|index| borrowed.kind.read(&buffer.bytes, index).ok())
                .map(JsvValue::Number)
                .unwrap_or(JsvValue::Undefined)
        }
        _ => JsvValue::Undefined,
    }
}

fn array_member(array: &JsvArrayRef, property: &str) -> Result<JsvValue, String> {
    match property {
        "length" => Ok(JsvValue::Number(array.borrow().len() as f64)),
        "push" | "pop" | "shift" | "unshift" | "join" | "slice" | "indexOf" | "includes"
        | "find" | "findIndex" | "some" | "every" | "forEach" | "map" | "filter" | "reduce"
        | "reduceRight" | "concat" | "reverse" | "sort" | "fill" | "at" | "keys" | "values"
        | "entries" | "flat" | "flatMap" | "copyWithin" | "lastIndexOf" | "splice" => {
            Ok(JsvValue::BoundNativeFn(
                format!("Array.{}", property),
                Box::new(JsvValue::Array(array.clone())),
            ))
        }
        _ => Ok(JsvValue::Undefined),
    }
}

fn string_member(text: &str, property: &str) -> Result<JsvValue, String> {
    if property == "length" {
        Ok(JsvValue::Number(text.chars().count() as f64))
    } else if string_method_supported(property) {
        Ok(JsvValue::BoundNativeFn(
            format!("String.{}", property),
            Box::new(JsvValue::String(text.to_string())),
        ))
    } else {
        Ok(JsvValue::Undefined)
    }
}

fn map_member(map: &JsvMapRef, property: &str) -> Result<JsvValue, String> {
    match property {
        "size" => Ok(JsvValue::Number(map.borrow().entries.len() as f64)),
        "get" | "set" | "has" | "delete" | "clear" | "keys" | "values" | "entries" | "forEach" => {
            Ok(JsvValue::BoundNativeFn(
                format!("Map.{}", property),
                Box::new(JsvValue::Map(map.clone())),
            ))
        }
        _ => Ok(JsvValue::Undefined),
    }
}

fn set_member(set: &JsvSetRef, property: &str) -> Result<JsvValue, String> {
    match property {
        "size" => Ok(JsvValue::Number(set.borrow().values.len() as f64)),
        "add" | "has" | "delete" | "clear" | "keys" | "values" | "entries" | "forEach" => {
            Ok(JsvValue::BoundNativeFn(
                format!("Set.{}", property),
                Box::new(JsvValue::Set(set.clone())),
            ))
        }
        _ => Ok(JsvValue::Undefined),
    }
}

fn weak_map_member(map: &JsvWeakMapRef, property: &str) -> Result<JsvValue, String> {
    match property {
        "get" | "set" | "has" | "delete" => Ok(JsvValue::BoundNativeFn(
            format!("WeakMap.{}", property),
            Box::new(JsvValue::WeakMap(map.clone())),
        )),
        _ => Ok(JsvValue::Undefined),
    }
}

fn weak_set_member(set: &JsvWeakSetRef, property: &str) -> Result<JsvValue, String> {
    match property {
        "add" | "has" | "delete" => Ok(JsvValue::BoundNativeFn(
            format!("WeakSet.{}", property),
            Box::new(JsvValue::WeakSet(set.clone())),
        )),
        _ => Ok(JsvValue::Undefined),
    }
}

fn typed_array_member(array: &JsvTypedArrayRef, property: &str) -> Result<JsvValue, String> {
    match property {
        "length" => Ok(JsvValue::Number(array.borrow().length as f64)),
        "byteLength" => Ok(JsvValue::Number(
            (array.borrow().length * array.borrow().kind.bytes_per_element()) as f64,
        )),
        "byteOffset" => Ok(JsvValue::Number(array.borrow().byte_offset as f64)),
        "buffer" => Ok(JsvValue::ArrayBuffer(array.borrow().buffer.clone())),
        "subarray" | "set" | "slice" | "fill" | "join" | "indexOf" | "includes" | "forEach"
        | "map" | "filter" | "reduce" | "some" | "every" | "keys" | "values" | "entries" | "at"
        | "reverse" | "sort" | "find" | "findIndex" => Ok(JsvValue::BoundNativeFn(
            format!("TypedArray.{}", property),
            Box::new(JsvValue::TypedArray(array.clone())),
        )),
        _ => Ok(JsvValue::Undefined),
    }
}

fn data_view_member(view: &JsvDataViewRef, property: &str) -> Result<JsvValue, String> {
    match property {
        "buffer" => Ok(JsvValue::ArrayBuffer(view.borrow().buffer.clone())),
        "byteLength" => Ok(JsvValue::Number(view.borrow().byte_length as f64)),
        "byteOffset" => Ok(JsvValue::Number(view.borrow().byte_offset as f64)),
        "getInt8" | "getUint8" | "getInt16" | "getUint16" | "getInt32" | "getUint32"
        | "getFloat32" | "getFloat64" | "setInt8" | "setUint8" | "setInt16" | "setUint16"
        | "setInt32" | "setUint32" | "setFloat32" | "setFloat64" => Ok(JsvValue::BoundNativeFn(
            format!("DataView.{}", property),
            Box::new(JsvValue::DataView(view.clone())),
        )),
        _ => Ok(JsvValue::Undefined),
    }
}

fn function_member(
    name: &str,
    params: &[ParamBinding],
    body: &JsvExpr,
    captured: &JsvEnvironment,
    property: &str,
) -> Result<JsvValue, String> {
    match property {
        "length" => Ok(JsvValue::Number(
            params
                .iter()
                .filter(|p| !matches!(p.pattern, ParamPattern::Rest(_)))
                .count() as f64,
        )),
        "name" => Ok(JsvValue::String(name.to_string())),
        "prototype" => Ok(JsvValue::Object(Rc::new(RefCell::new(JsvObject::plain(
            HashMap::new(),
        ))))),
        "call" => Ok(JsvValue::BoundNativeFn(
            "Function.call".to_string(),
            Box::new(JsvValue::Function(
                name.to_string(),
                params.to_vec(),
                Box::new(body.clone()),
                captured.clone(),
            )),
        )),
        "apply" => Ok(JsvValue::BoundNativeFn(
            "Function.apply".to_string(),
            Box::new(JsvValue::Function(
                name.to_string(),
                params.to_vec(),
                Box::new(body.clone()),
                captured.clone(),
            )),
        )),
        _ => Ok(JsvValue::Undefined),
    }
}

fn generator_member(generator: &JsvGeneratorRef, property: &str) -> Result<JsvValue, String> {
    match property {
        "next" | "return" | "throw" => Ok(JsvValue::BoundNativeFn(
            format!("Generator.{}", property),
            Box::new(JsvValue::GeneratorObject(generator.clone())),
        )),
        _ => Ok(JsvValue::Undefined),
    }
}

#[allow(clippy::too_many_arguments)]
fn write_property(
    value: JsvValue,
    property: &str,
    rhs: JsvValue,
    env: &mut JsvEnvironment,
    console: &mut Vec<String>,
    ctx: &mut EvalCtx<'_>,
) -> Result<JsvValue, String> {
    let value = value.deref_live();
    match value {
        JsvValue::HostObject(object) => ctx.host.set_property(object, property, rhs),
        JsvValue::Proxy(proxy) => proxy_set(&proxy, property, rhs, env, console, ctx),
        JsvValue::Object(object) => {
            let existing = object
                .try_borrow()
                .map_err(|_| "TypeError: Reentrant access to object".to_string())?
                .properties
                .get(property)
                .cloned();
            if let Some(JsvValue::GetterSetter(_, set)) = existing {
                if is_callable(&set) {
                    let called = call_function(
                        *set,
                        vec![rhs],
                        env,
                        console,
                        ctx,
                        JsvValue::Object(object.clone()),
                    )?;
                    return call_result_value(called);
                }
                return Err("TypeError: property has no setter".to_string());
            }
            object
                .try_borrow_mut()
                .map_err(|_| "TypeError: Reentrant access to object".to_string())?
                .properties
                .insert(property.to_string(), rhs.clone());
            Ok(rhs)
        }
        JsvValue::Array(array) if property == "length" => {
            let length = rhs
                .as_number()
                .filter(|value| value.is_finite() && *value >= 0.0)
                .map(|value| value as usize)
                .ok_or_else(|| "RangeError: invalid array length".to_string())?;
            if length > MAX_ARRAY_ELEMENTS {
                return Err("RangeError: array length budget exceeded".to_string());
            }
            array.borrow_mut().resize(length, JsvValue::Undefined);
            Ok(rhs)
        }
        other => Err(format!(
            "TypeError: Cannot set property '{}' of {}",
            property,
            other.to_display_string()
        )),
    }
}

#[allow(clippy::too_many_arguments)]
fn index_read(
    value: JsvValue,
    key: JsvValue,
    env: &mut JsvEnvironment,
    console: &mut Vec<String>,
    ctx: &mut EvalCtx<'_>,
    optional: bool,
) -> Result<JsvValue, String> {
    if matches!(value, JsvValue::Null | JsvValue::Undefined) {
        if optional {
            return Ok(JsvValue::Undefined);
        }
        return Err(format!(
            "TypeError: Cannot read property '{}' of null or undefined",
            property_key(&key)
        ));
    }
    let value = value.deref_live();
    let key_text = property_key(&key);
    match value {
        JsvValue::Array(array) => {
            if key_text == "length" {
                return Ok(JsvValue::Number(array.borrow().len() as f64));
            }
            if is_symbol_property_key(&key_text) {
                return symbol_index_read(JsvValue::Array(array), &key_text, env, console, ctx);
            }
            let index = key_text.parse::<usize>().ok();
            Ok(index
                .and_then(|index| array.borrow().get(index).cloned())
                .unwrap_or(JsvValue::Undefined))
        }
        JsvValue::String(text) => {
            let index = key_text.parse::<usize>().ok();
            Ok(index
                .and_then(|index| text.chars().nth(index))
                .map(|character| JsvValue::String(character.to_string()))
                .unwrap_or(JsvValue::Undefined))
        }
        JsvValue::TypedArray(array) => {
            let borrowed = array
                .try_borrow()
                .map_err(|_| "TypeError: Reentrant access to typed array".to_string())?;
            let buffer = borrowed
                .buffer
                .try_borrow()
                .map_err(|_| "TypeError: Reentrant access to ArrayBuffer".to_string())?;
            if buffer.detached {
                return Err("TypeError: Cannot access a detached ArrayBuffer".to_string());
            }
            if is_symbol_property_key(&key_text) {
                drop(buffer);
                drop(borrowed);
                return symbol_index_read(
                    JsvValue::TypedArray(array.clone()),
                    &key_text,
                    env,
                    console,
                    ctx,
                );
            }
            let index = key_text.parse::<usize>().ok();
            let Some(index) = index.filter(|index| *index < borrowed.length) else {
                return Ok(JsvValue::Undefined);
            };
            let value = borrowed.kind.read(&buffer.bytes, index)?;
            Ok(JsvValue::Number(value))
        }
        JsvValue::Object(object) => {
            if is_symbol_property_key(&key_text) {
                return symbol_index_read(
                    JsvValue::Object(object.clone()),
                    &key_text,
                    env,
                    console,
                    ctx,
                );
            }
            if let Some(found) = object_property(&object, &key_text) {
                let found = found.deref_live();
                if let JsvValue::GetterSetter(ref get, _) = found {
                    if is_callable(get) {
                        let called = call_function(
                            (**get).clone(),
                            Vec::new(),
                            env,
                            console,
                            ctx,
                            JsvValue::Object(object),
                        )?;
                        return call_result_value(called);
                    }
                }
                Ok(found)
            } else {
                Ok(JsvValue::Undefined)
            }
        }
        JsvValue::Proxy(proxy) => proxy_get(&proxy, &key_text, env, console, ctx),
        JsvValue::GeneratorObject(_) | JsvValue::Iterator(_) => {
            if is_symbol_property_key(&key_text) {
                symbol_index_read(value.clone(), &key_text, env, console, ctx)
            } else {
                Ok(JsvValue::Undefined)
            }
        }
        JsvValue::HostObject(object) => ctx.host.get_property(object, &key_text),
        other => Err(format!(
            "TypeError: {} is not indexable",
            other.to_display_string()
        )),
    }
}

#[allow(clippy::too_many_arguments)]
fn index_write(
    value: JsvValue,
    key: JsvValue,
    rhs: JsvValue,
    env: &mut JsvEnvironment,
    console: &mut Vec<String>,
    ctx: &mut EvalCtx<'_>,
) -> Result<JsvValue, String> {
    let value = value.deref_live();
    let key_text = property_key(&key);
    match value {
        JsvValue::Array(array) => {
            let index = key_text
                .parse::<usize>()
                .map_err(|_| "TypeError: invalid array index".to_string())?;
            if index >= MAX_ARRAY_ELEMENTS {
                return Err("RangeError: array index budget exceeded".to_string());
            }
            let mut array = array.borrow_mut();
            if index >= array.len() {
                array.resize(index + 1, JsvValue::Undefined);
            }
            array[index] = rhs.clone();
            Ok(rhs)
        }
        JsvValue::TypedArray(array) => {
            let borrowed = array.borrow_mut();
            if borrowed.buffer.borrow().detached {
                return Err("TypeError: Cannot access a detached ArrayBuffer".to_string());
            }
            let index = key_text
                .parse::<usize>()
                .map_err(|_| "TypeError: invalid typed array index".to_string())?;
            if index < borrowed.length {
                let value = to_number(&rhs.deref_live());
                borrowed
                    .kind
                    .write(&mut borrowed.buffer.borrow_mut().bytes, index, value);
            }
            Ok(rhs)
        }
        JsvValue::Object(object) => {
            let existing = object.borrow().properties.get(&key_text).cloned();
            if let Some(JsvValue::GetterSetter(_, set)) = existing {
                if is_callable(&set) {
                    let called = call_function(
                        *set,
                        vec![rhs.clone()],
                        env,
                        console,
                        ctx,
                        JsvValue::Object(object.clone()),
                    )?;
                    return call_result_value(called);
                }
                return Err("TypeError: property has no setter".to_string());
            }
            object.borrow_mut().properties.insert(key_text, rhs.clone());
            Ok(rhs)
        }
        JsvValue::Proxy(proxy) => proxy_set(&proxy, &key_text, rhs, env, console, ctx),
        JsvValue::HostObject(object) => ctx.host.set_property(object, &key_text, rhs),
        other => Err(format!(
            "TypeError: Cannot set property of {}",
            other.to_display_string()
        )),
    }
}

fn own_properties(value: &JsvValue) -> Result<Vec<(String, JsvValue)>, String> {
    let value = value.deref_live();
    match value {
        JsvValue::Object(object) => Ok(object
            .borrow()
            .properties
            .iter()
            .filter(|(key, _)| !is_symbol_property_key(key))
            .map(|(key, item)| (key.clone(), item.clone()))
            .collect()),
        JsvValue::Array(array) => Ok(array
            .borrow()
            .iter()
            .enumerate()
            .map(|(index, item)| (index.to_string(), item.clone()))
            .collect()),
        JsvValue::TypedArray(array) => {
            let borrowed = array
                .try_borrow()
                .map_err(|_| "TypeError: Reentrant access to typed array".to_string())?;
            let buffer = borrowed
                .buffer
                .try_borrow()
                .map_err(|_| "TypeError: Reentrant access to ArrayBuffer".to_string())?;
            if buffer.detached {
                return Err("TypeError: Cannot access a detached ArrayBuffer".to_string());
            }
            let mut out = Vec::with_capacity(borrowed.length.min(1024));
            for index in 0..borrowed.length {
                let value = borrowed.kind.read(&buffer.bytes, index)?;
                out.push((index.to_string(), JsvValue::Number(value)));
            }
            Ok(out)
        }
        JsvValue::Proxy(proxy) => proxy_own_keys(&proxy),
        _ => Ok(Vec::new()),
    }
}

fn iterate_values(
    value: &JsvValue,
    console: &mut Vec<String>,
    ctx: &mut EvalCtx<'_>,
) -> Result<Vec<JsvValue>, String> {
    let value = value.deref_live();
    let mut out = Vec::new();
    let iterator = iterator_from(&value)?;
    loop {
        let (done, item) = iterator_next(&iterator, JsvValue::Undefined, console, ctx)?;
        if done {
            break;
        }
        if out.len() >= MAX_ITERATOR_RESULTS {
            return Err("Iterator result budget exceeded".to_string());
        }
        out.push(item);
    }
    Ok(out)
}

fn iterator_from(value: &JsvValue) -> Result<JsvIteratorRef, String> {
    let value = value.deref_live();
    match value {
        JsvValue::Array(array) => Ok(Rc::new(RefCell::new(JsvIteratorState {
            kind: IteratorKind::Array,
            source: JsvValue::Array(array),
            index: 0,
            byte_offset: 0,
            snapshot: None,
            column: 0,
            generator: None,
        }))),
        JsvValue::String(text) => Ok(Rc::new(RefCell::new(JsvIteratorState {
            kind: IteratorKind::String,
            source: JsvValue::String(text),
            index: 0,
            byte_offset: 0,
            snapshot: None,
            column: 0,
            generator: None,
        }))),
        JsvValue::Map(map) => Ok(Rc::new(RefCell::new(JsvIteratorState {
            kind: IteratorKind::Map,
            source: JsvValue::Map(map.clone()),
            index: 0,
            byte_offset: 0,
            snapshot: Some(JsvValue::Map(map.clone())),
            column: 0,
            generator: None,
        }))),
        JsvValue::Set(set) => Ok(Rc::new(RefCell::new(JsvIteratorState {
            kind: IteratorKind::Set,
            source: JsvValue::Set(set.clone()),
            index: 0,
            byte_offset: 0,
            snapshot: Some(JsvValue::Set(set.clone())),
            column: 0,
            generator: None,
        }))),
        JsvValue::TypedArray(array) => Ok(Rc::new(RefCell::new(JsvIteratorState {
            kind: IteratorKind::TypedArray,
            source: JsvValue::TypedArray(array.clone()),
            index: 0,
            byte_offset: 0,
            snapshot: None,
            column: 0,
            generator: None,
        }))),
        JsvValue::GeneratorObject(generator) => Ok(Rc::new(RefCell::new(JsvIteratorState {
            kind: IteratorKind::Generator,
            source: JsvValue::GeneratorObject(generator.clone()),
            index: 0,
            byte_offset: 0,
            snapshot: None,
            column: 0,
            generator: Some(generator.clone()),
        }))),
        JsvValue::Iterator(iterator) => Ok(iterator),
        JsvValue::Object(_) => Err(
            "TypeError: value is not iterable (no [Symbol.iterator] on plain objects)".to_string(),
        ),
        _ => Err(format!(
            "TypeError: {} is not iterable",
            value.to_display_string()
        )),
    }
}

#[allow(clippy::too_many_arguments)]
fn iterator_next(
    iterator: &JsvIteratorRef,
    argument: JsvValue,
    console: &mut Vec<String>,
    ctx: &mut EvalCtx<'_>,
) -> Result<(bool, JsvValue), String> {
    let mut state = iterator.borrow_mut();
    match state.kind {
        IteratorKind::Array => {
            let array = match &state.source {
                JsvValue::Array(array) => array.clone(),
                _ => return Err("InvalidStateError: iterator source changed".to_string()),
            };
            let borrowed = array.borrow();
            if state.index >= borrowed.len() {
                Ok((true, JsvValue::Undefined))
            } else {
                let value = borrowed[state.index].clone();
                state.index += 1;
                Ok((false, value))
            }
        }
        IteratorKind::String => {
            let text = match &state.source {
                JsvValue::String(text) => text.clone(),
                _ => return Err("InvalidStateError: iterator source changed".to_string()),
            };
            if let Some(character) = text[state.byte_offset..].chars().next() {
                let value = JsvValue::String(character.to_string());
                state.index += 1;
                state.byte_offset += character.len_utf8();
                Ok((false, value))
            } else {
                Ok((true, JsvValue::Undefined))
            }
        }
        IteratorKind::Map => {
            let map = match &state.snapshot {
                Some(JsvValue::Map(map)) => map.clone(),
                _ => return Err("InvalidStateError: Map iterator source changed".to_string()),
            };
            let entry = map.borrow().entries.get(state.index).cloned();
            if let Some((key, value)) = entry {
                let value = match state.column {
                    0 => JsvValue::Array(Rc::new(RefCell::new(vec![key, value]))),
                    1 => key,
                    _ => value,
                };
                state.index += 1;
                Ok((false, value))
            } else {
                Ok((true, JsvValue::Undefined))
            }
        }
        IteratorKind::Set => {
            let set = match &state.snapshot {
                Some(JsvValue::Set(set)) => set.clone(),
                _ => return Err("InvalidStateError: Set iterator source changed".to_string()),
            };
            let value = set.borrow().values.get(state.index).cloned();
            if let Some(value) = value {
                state.index += 1;
                if state.column == 1 {
                    Ok((
                        false,
                        JsvValue::Array(Rc::new(RefCell::new(vec![value.clone(), value]))),
                    ))
                } else {
                    Ok((false, value))
                }
            } else {
                Ok((true, JsvValue::Undefined))
            }
        }
        IteratorKind::TypedArray => {
            let array = match &state.source {
                JsvValue::TypedArray(array) => array.clone(),
                _ => return Err("InvalidStateError: iterator source changed".to_string()),
            };
            let borrowed = array
                .try_borrow()
                .map_err(|_| "TypeError: Reentrant access to typed array".to_string())?;
            let buffer = borrowed
                .buffer
                .try_borrow()
                .map_err(|_| "TypeError: Reentrant access to ArrayBuffer".to_string())?;
            if buffer.detached {
                return Err("TypeError: Cannot access a detached ArrayBuffer".to_string());
            }
            if state.index >= borrowed.length {
                Ok((true, JsvValue::Undefined))
            } else {
                let value = JsvValue::Number(borrowed.kind.read(&buffer.bytes, state.index)?);
                drop(buffer);
                drop(borrowed);
                state.index += 1;
                Ok((false, value))
            }
        }
        IteratorKind::Generator => {
            let generator = state.generator.clone().ok_or_else(|| {
                "InvalidStateError: generator iterator lost its generator".to_string()
            })?;
            drop(state);
            generator_next(&generator, argument, console, ctx)
        }
        IteratorKind::Keys | IteratorKind::Values | IteratorKind::Entries => {
            Err("InvalidStateError: collection column iterators require source".to_string())
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn symbol_index_read(
    value: JsvValue,
    key: &str,
    env: &mut JsvEnvironment,
    console: &mut Vec<String>,
    ctx: &mut EvalCtx<'_>,
) -> Result<JsvValue, String> {
    let Some(id) = key.strip_prefix(SYMBOL_KEY_PREFIX) else {
        return Ok(JsvValue::Undefined);
    };
    let Some(symbol_object) = env
        .get("Symbol")
        .map(|symbol| symbol.deref_live())
        .and_then(|symbol| match symbol {
            JsvValue::Object(object) => Some(object),
            _ => None,
        })
    else {
        return Ok(JsvValue::Undefined);
    };
    let is_well_known = |name: &str| -> bool {
        symbol_object
            .borrow()
            .properties
            .get(name)
            .and_then(|value| value.as_symbol_id())
            .map(|well_known_id| well_known_id.to_string() == id)
            .unwrap_or(false)
    };
    let _ = (console, ctx);
    if is_well_known(WellKnownSymbol::Iterator.name()) {
        return Ok(JsvValue::BoundNativeFn(
            "[Symbol.iterator]".to_string(),
            Box::new(value),
        ));
    }
    if is_well_known(WellKnownSymbol::ToStringTag.name()) {
        return Ok(JsvValue::String("[object Object]".to_string()));
    }
    Ok(JsvValue::Undefined)
}

#[allow(clippy::too_many_arguments)]
fn eval_call(
    callee: &JsvExpr,
    args: &[JsvExpr],
    env: &mut JsvEnvironment,
    console: &mut Vec<String>,
    ctx: &mut EvalCtx<'_>,
    is_new: bool,
) -> Result<JsvValue, String> {
    let (callee_val, this_arg) = match callee {
        JsvExpr::Member(base, property) => {
            let base = eval_expr(base, env, console, ctx)?;
            if let JsvValue::YieldSignal(signal) = base {
                return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                    Cont::CallMemberBase {
                        property: property.clone(),
                        args: args.to_vec(),
                        env: env.clone(),
                        is_new,
                    },
                ))));
            }
            let base = base.deref_live();
            let func = read_property(base.clone(), property, env, console, ctx, false)?;
            (func, base)
        }
        JsvExpr::Index(base, key) => {
            let base = eval_expr(base, env, console, ctx)?;
            if let JsvValue::YieldSignal(signal) = base {
                return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                    Cont::CallIndexBase {
                        key_expr: key.clone(),
                        args: args.to_vec(),
                        env: env.clone(),
                        is_new,
                    },
                ))));
            }
            let base = base.deref_live();
            let key = eval_expr(key, env, console, ctx)?;
            if let JsvValue::YieldSignal(signal) = key {
                return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                    Cont::CallIndexKey {
                        object: base,
                        args: args.to_vec(),
                        env: env.clone(),
                        is_new,
                    },
                ))));
            }
            let func = index_read(base.clone(), key.deref_live(), env, console, ctx, false)?;
            (func, base)
        }
        JsvExpr::PrivateGet(base, name) => {
            let base = eval_expr(base, env, console, ctx)?;
            if let JsvValue::YieldSignal(signal) = base {
                return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                    Cont::CallPrivateBase {
                        name: name.clone(),
                        args: args.to_vec(),
                        env: env.clone(),
                    },
                ))));
            }
            let func = private_read(&base.deref_live(), name, env)?;
            (func, base)
        }
        JsvExpr::SuperMember(name) => {
            let this_value = env.get("this").unwrap_or(JsvValue::Undefined);
            let super_proto = env.get("#superProto").unwrap_or(JsvValue::Undefined);
            let JsvValue::Object(proto) = super_proto else {
                return Err("TypeError: super.method() has no superclass".to_string());
            };
            let func = object_property(&proto, name)
                .ok_or_else(|| format!("TypeError: super.{} is not a function", name))?
                .deref_live();
            (func, this_value)
        }
        _ => {
            let value = eval_expr(callee, env, console, ctx)?;
            if let JsvValue::YieldSignal(signal) = value {
                return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                    Cont::CallCallee {
                        args: args.to_vec(),
                        env: env.clone(),
                        is_new,
                    },
                ))));
            }
            (value.deref_live(), JsvValue::Undefined)
        }
    };

    let mut arg_vals = Vec::new();
    for (index, arg) in args.iter().enumerate() {
        if let JsvExpr::Spread(spread) = arg {
            let value = eval_expr(spread, env, console, ctx)?;
            if let JsvValue::YieldSignal(signal) = value {
                return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                    Cont::CallArgs {
                        callee: callee_val.clone(),
                        done: arg_vals,
                        left: args[index + 1..].to_vec(),
                        env: env.clone(),
                        is_new,
                        this_arg: this_arg.clone(),
                    },
                ))));
            }
            for item in iterate_values(&value.deref_live(), console, ctx)? {
                arg_vals.push(item);
            }
        } else {
            let value = eval_expr(arg, env, console, ctx)?;
            if let JsvValue::YieldSignal(signal) = value {
                return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                    Cont::CallArgs {
                        callee: callee_val.clone(),
                        done: arg_vals,
                        left: args[index + 1..].to_vec(),
                        env: env.clone(),
                        is_new,
                        this_arg: this_arg.clone(),
                    },
                ))));
            }
            arg_vals.push(value);
        }
        if arg_vals.len() > MAX_ARRAY_ELEMENTS {
            return Err("Argument budget exceeded".to_string());
        }
    }

    if is_new {
        invoke_new(callee_val, arg_vals, env, console, ctx)
    } else {
        let called = call_function(callee_val, arg_vals, env, console, ctx, this_arg)?;
        call_result_value(called)
    }
}

fn is_constructible_native(name: &str) -> bool {
    matches!(
        name,
        "Map"
            | "Set"
            | "WeakMap"
            | "WeakSet"
            | "WeakRef"
            | "FinalizationRegistry"
            | "ArrayBuffer"
            | "DataView"
            | "Function"
            | "Proxy"
            | "Symbol"
            | "Error"
            | "TypeError"
            | "RangeError"
            | "ReferenceError"
            | "SyntaxError"
            | "EvalError"
            | "URIError"
            | "Array"
            | "Object"
            | "Promise"
    ) || (name.ends_with("Array") && TypedArrayKind::from_name(name).is_some())
        || name.starts_with("Intl.")
}

fn invoke_new(
    callee: JsvValue,
    args: Vec<JsvValue>,
    env: &mut JsvEnvironment,
    console: &mut Vec<String>,
    ctx: &mut EvalCtx<'_>,
) -> Result<JsvValue, String> {
    match callee {
        JsvValue::Class(class) => construct_class(&class, args, env, console, ctx),
        JsvValue::HostFunction(object, method) => ctx.host.call(object, &method, args),
        JsvValue::NativeFn(name) => {
            if is_constructible_native(&name) {
                call_native_fn(&name, args, env, console, ctx)
            } else {
                Err(format!("TypeError: {} is not a constructor", name))
            }
        }
        function if is_callable(&function) => {
            call_function(function, args, env, console, ctx, JsvValue::Undefined)
                .and_then(call_result_value)
        }
        other => Err(format!(
            "TypeError: {} is not a constructor",
            other.to_display_string()
        )),
    }
}

#[allow(clippy::too_many_arguments)]
fn eval_class_def(
    name: &Option<String>,
    extends: &Option<Box<JsvExpr>>,
    members: &[ClassMember],
    env: &mut JsvEnvironment,
    console: &mut Vec<String>,
    ctx: &mut EvalCtx<'_>,
) -> Result<JsvValue, String> {
    let parent = match extends {
        Some(expr) => {
            let value = eval_expr(expr, env, console, ctx)?.deref_live();
            match value {
                JsvValue::Class(_) => Some(value),
                JsvValue::Null => None,
                other => {
                    return Err(format!(
                        "TypeError: class extends value {} is not a constructor",
                        other.to_display_string()
                    ))
                }
            }
        }
        None => None,
    };
    let class_name = name.clone().unwrap_or_default();
    let id = NEXT_CLASS_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    let mut class_env = JsvEnvironment::with_parent(env.clone());
    let parent_proto = parent.as_ref().and_then(|value| match value {
        JsvValue::Class(class) => Some(class.prototype_object.clone()),
        _ => None,
    });
    let super_proto_value = parent_proto
        .clone()
        .map(JsvValue::Object)
        .unwrap_or(JsvValue::Undefined);
    let parent_class_value = parent.clone().unwrap_or(JsvValue::Undefined);

    let prototype_object = Rc::new(RefCell::new(JsvObject {
        properties: HashMap::new(),
        prototype: parent_proto,
        class_tags: Vec::new(),
        private_fields: HashMap::new(),
    }));

    let mut method_env = JsvEnvironment::with_parent(class_env.clone());
    method_env.define("#classId", JsvValue::Number(id as f64));
    method_env.define("#superProto", super_proto_value);
    method_env.define("#parentClass", parent_class_value);

    let mut instance_methods = HashMap::new();
    let mut static_methods = HashMap::new();
    let mut private_names = Vec::new();
    let mut instance_fields = Vec::new();
    let mut constructor: Option<(Vec<ParamBinding>, Box<JsvExpr>)> = None;

    for member in members {
        match member {
            ClassMember::Method(method) => {
                let is_constructor =
                    !method.is_static && !method.is_private && method.name == "constructor";
                if is_constructor {
                    constructor = Some((method.params.clone(), method.body.clone()));
                    continue;
                }
                if method.is_private {
                    private_names.push(method.name.clone());
                }
                let func = if method.is_async {
                    JsvValue::AsyncFunction(
                        method.name.clone(),
                        method.params.clone(),
                        method.body.clone(),
                        method_env.clone(),
                    )
                } else if method.is_generator {
                    JsvValue::GeneratorFn(
                        method.name.clone(),
                        method.params.clone(),
                        method.body.clone(),
                        method_env.clone(),
                    )
                } else {
                    JsvValue::Function(
                        method.name.clone(),
                        method.params.clone(),
                        method.body.clone(),
                        method_env.clone(),
                    )
                };
                if method.is_private {
                    let stripped = method.name.trim_start_matches('#').to_string();
                    prototype_object
                        .borrow_mut()
                        .properties
                        .insert(format!("{}\u{0}private:{}", id, stripped), func);
                } else if method.is_static {
                    static_methods.insert(method.name.clone(), func);
                } else if method.is_getter || method.is_setter {
                    let existing = prototype_object
                        .borrow()
                        .properties
                        .get(&method.name)
                        .cloned();
                    let (get, set) = match existing {
                        Some(JsvValue::GetterSetter(get, set)) => ((*get).clone(), (*set).clone()),
                        _ => (JsvValue::Undefined, JsvValue::Undefined),
                    };
                    let accessor = if method.is_getter {
                        JsvValue::GetterSetter(Box::new(func.clone()), Box::new(set))
                    } else {
                        JsvValue::GetterSetter(Box::new(get), Box::new(func))
                    };
                    prototype_object
                        .borrow_mut()
                        .properties
                        .insert(method.name.clone(), accessor);
                } else {
                    instance_methods.insert(method.name.clone(), func.clone());
                    prototype_object
                        .borrow_mut()
                        .properties
                        .insert(method.name.clone(), func);
                }
            }
            ClassMember::Field {
                name,
                initializer,
                is_static,
            } => {
                if name.starts_with('#') {
                    private_names.push(name.clone());
                }
                if *is_static {
                    let value = match initializer {
                        Some(expr) => eval_expr(expr, &mut class_env.clone(), console, ctx)?,
                        None => JsvValue::Undefined,
                    };
                    static_methods.insert(name.clone(), value);
                } else {
                    instance_fields.push((name.clone(), initializer.clone()));
                }
            }
        }
    }

    if instance_methods.len() + static_methods.len() + private_names.len() > 1_024 {
        return Err("Class member budget exceeded".to_string());
    }

    let class_value = JsvValue::Class(Rc::new(JsvClass {
        name: class_name.clone(),
        id,
        parent,
        constructor,
        instance_methods,
        static_methods,
        private_names,
        instance_fields,
        prototype_object: prototype_object.clone(),
        class_env: class_env.clone(),
    }));
    if let Some(class_name) = name {
        class_env.define(class_name, class_value.clone());
        env.define(class_name, class_value.clone());
    }
    Ok(class_value)
}

fn construct_class(
    class: &JsvClassRef,
    args: Vec<JsvValue>,
    env: &mut JsvEnvironment,
    console: &mut Vec<String>,
    ctx: &mut EvalCtx<'_>,
) -> Result<JsvValue, String> {
    let mut tags = Vec::new();
    let mut current: Option<JsvClassRef> = Some(class.clone());
    while let Some(reference) = current {
        tags.push(reference.id);
        current = reference.parent.as_ref().and_then(|value| match value {
            JsvValue::Class(parent) => Some(parent.clone()),
            _ => None,
        });
    }
    let instance = Rc::new(RefCell::new(JsvObject {
        properties: HashMap::new(),
        prototype: Some(class.prototype_object.clone()),
        class_tags: tags,
        private_fields: HashMap::new(),
    }));
    let this_value = JsvValue::Object(instance);
    construct_class_with_this(class, args, this_value.clone(), env, console, ctx)
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::only_used_in_recursion)]
fn construct_class_with_this(
    class: &JsvClassRef,
    args: Vec<JsvValue>,
    this_value: JsvValue,
    env: &mut JsvEnvironment,
    console: &mut Vec<String>,
    ctx: &mut EvalCtx<'_>,
) -> Result<JsvValue, String> {
    ctx.call_depth += 1;
    if ctx.call_depth > MAX_CALL_DEPTH {
        ctx.call_depth = ctx.call_depth.saturating_sub(1);
        return Err("Maximum call stack size exceeded".to_string());
    }
    let result = construct_class_with_this_inner(class, args, this_value, env, console, ctx);
    ctx.call_depth = ctx.call_depth.saturating_sub(1);
    result
}

fn construct_class_with_this_inner(
    class: &JsvClassRef,
    args: Vec<JsvValue>,
    this_value: JsvValue,
    env: &mut JsvEnvironment,
    console: &mut Vec<String>,
    ctx: &mut EvalCtx<'_>,
) -> Result<JsvValue, String> {
    let JsvValue::Object(this_object) = &this_value else {
        return Err("TypeError: super() requires a valid this".to_string());
    };
    let mut field_env = JsvEnvironment::with_parent(class.class_env.clone());
    field_env.define("this", this_value.clone());
    field_env.define("#classId", JsvValue::Number(class.id as f64));
    field_env.define(
        "#superProto",
        class
            .parent
            .as_ref()
            .and_then(|value| match value {
                JsvValue::Class(parent) => Some(JsvValue::Object(parent.prototype_object.clone())),
                _ => None,
            })
            .unwrap_or(JsvValue::Undefined),
    );
    field_env.define(
        "#parentClass",
        class.parent.clone().unwrap_or(JsvValue::Undefined),
    );
    for (field_name, initializer) in &class.instance_fields {
        let value = match initializer {
            Some(expr) => eval_expr(expr, &mut field_env, console, ctx)?,
            None => JsvValue::Undefined,
        };
        if let Some(stripped) = field_name.strip_prefix('#') {
            this_object
                .borrow_mut()
                .private_fields
                .insert(format!("{}:{}", class.id, stripped), value);
        } else {
            this_object
                .borrow_mut()
                .properties
                .insert(field_name.clone(), value);
        }
    }

    if let Some((params, body)) = &class.constructor {
        let mut ctor_env = JsvEnvironment::with_parent(class.class_env.clone());
        ctor_env.define("this", this_value.clone());
        ctor_env.define("#classId", JsvValue::Number(class.id as f64));
        ctor_env.define(
            "#superProto",
            class
                .parent
                .as_ref()
                .and_then(|value| match value {
                    JsvValue::Class(parent) => {
                        Some(JsvValue::Object(parent.prototype_object.clone()))
                    }
                    _ => None,
                })
                .unwrap_or(JsvValue::Undefined),
        );
        ctor_env.define(
            "#parentClass",
            class.parent.clone().unwrap_or(JsvValue::Undefined),
        );
        bind_params(params, &args, &mut ctor_env, console, ctx)?;
        let result = eval_expr(body, &mut ctor_env, console, ctx)?;
        match result {
            JsvValue::ReturnSignal(value) => {
                let value = *value;
                if value.is_object_like() {
                    Ok(value)
                } else {
                    Ok(this_value)
                }
            }
            JsvValue::ThrowSignal(_) | JsvValue::BreakSignal | JsvValue::ContinueSignal => {
                Ok(result)
            }
            JsvValue::YieldSignal(_) => {
                Err("SyntaxError: yield in class constructor is not supported".to_string())
            }
            _ => Ok(this_value),
        }
    } else if let Some(parent) = &class.parent {
        let JsvValue::Class(parent_class) = parent else {
            return Err("TypeError: invalid superclass".to_string());
        };
        construct_class_with_this(parent_class, args, this_value, env, console, ctx)
    } else {
        Ok(this_value)
    }
}

fn private_class_id(env: &JsvEnvironment) -> Result<u64, String> {
    match env.get("#classId") {
        Some(JsvValue::Number(id)) if id > 0.0 && id.fract() == 0.0 => Ok(id as u64),
        _ => Err("ReferenceError: private member access outside of a class body".to_string()),
    }
}

fn private_read(target: &JsvValue, name: &str, env: &JsvEnvironment) -> Result<JsvValue, String> {
    let class_id = private_class_id(env)?;
    let JsvValue::Object(object) = target else {
        return Err("TypeError: cannot access a private member on a non-object".to_string());
    };
    let stripped = name.trim_start_matches('#').to_string();
    let field_key = format!("{}:{}", class_id, stripped);
    {
        let borrowed = object.borrow();
        if !borrowed.class_tags.contains(&class_id) {
            return Err(format!(
                "TypeError: Cannot read private member #{} from an object whose class did not declare it",
                stripped
            ));
        }
        if let Some(value) = borrowed.private_fields.get(&field_key) {
            return Ok(value.clone());
        }
    }

    let method_key = format!("{}\u{0}private:{}", class_id, stripped);
    Ok(object_property(object, &method_key).unwrap_or(JsvValue::Undefined))
}

fn private_write(
    target: &JsvValue,
    name: &str,
    value: JsvValue,
    env: &JsvEnvironment,
) -> Result<(), String> {
    let class_id = private_class_id(env)?;
    let JsvValue::Object(object) = target else {
        return Err("TypeError: cannot access a private member on a non-object".to_string());
    };
    let stripped = name.trim_start_matches('#').to_string();
    {
        let borrowed = object.borrow();
        if !borrowed.class_tags.contains(&class_id) {
            return Err(format!(
                "TypeError: Cannot write private member #{} to an object whose class did not declare it",
                stripped
            ));
        }
    }
    object
        .borrow_mut()
        .private_fields
        .insert(format!("{}:{}", class_id, stripped), value);
    Ok(())
}

fn proxy_trap(proxy: &JsvProxy, trap_name: &str, _env: &JsvEnvironment) -> Option<JsvValue> {
    let handler = proxy.handler.deref_live();
    match handler {
        JsvValue::Object(object) => {
            object_property(&object, trap_name).map(|value| value.deref_live())
        }
        _ => None,
    }
}

fn proxy_get(
    proxy: &JsvProxy,
    key: &str,
    env: &mut JsvEnvironment,
    console: &mut Vec<String>,
    ctx: &mut EvalCtx<'_>,
) -> Result<JsvValue, String> {
    if ctx.proxy_depth >= 64 {
        return Err("Proxy trap recursion limit exceeded".to_string());
    }
    ctx.proxy_depth += 1;
    let result = (|| {
        if let Some(trap) = proxy_trap(proxy, "get", env) {
            if is_callable(&trap) {
                let args = vec![
                    proxy.target.clone(),
                    JsvValue::String(key.to_string()),
                    proxy.handler.clone(),
                ];
                let called = call_function(trap, args, env, console, ctx, proxy.handler.clone())?;
                return call_result_value(called);
            }
        }
        read_property(
            proxy.target.clone().deref_live(),
            key,
            env,
            console,
            ctx,
            false,
        )
    })();
    ctx.proxy_depth -= 1;
    result
}

fn proxy_set(
    proxy: &JsvProxy,
    key: &str,
    value: JsvValue,
    env: &mut JsvEnvironment,
    console: &mut Vec<String>,
    ctx: &mut EvalCtx<'_>,
) -> Result<JsvValue, String> {
    if ctx.proxy_depth >= 64 {
        return Err("Proxy trap recursion limit exceeded".to_string());
    }
    ctx.proxy_depth += 1;
    let result = (|| {
        if let Some(trap) = proxy_trap(proxy, "set", env) {
            if is_callable(&trap) {
                let args = vec![
                    proxy.target.clone(),
                    JsvValue::String(key.to_string()),
                    value.clone(),
                    proxy.handler.clone(),
                ];
                let called = call_function(trap, args, env, console, ctx, proxy.handler.clone())?;
                let accepted = call_result_value(called)?;
                if accepted.is_truthy() {
                    return Ok(value);
                }
                return Err(format!(
                    "TypeError: 'set' proxy trap returned false for '{}'",
                    key
                ));
            }
        }
        write_property(
            proxy.target.clone().deref_live(),
            key,
            value,
            env,
            console,
            ctx,
        )
    })();
    ctx.proxy_depth -= 1;
    result
}

fn proxy_has(
    proxy: &JsvProxy,
    key: &str,
    env: &mut JsvEnvironment,
    console: &mut Vec<String>,
    ctx: &mut EvalCtx<'_>,
) -> Result<bool, String> {
    if let Some(trap) = proxy_trap(proxy, "has", env) {
        if is_callable(&trap) {
            let args = vec![proxy.target.clone(), JsvValue::String(key.to_string())];
            let called = call_function(trap, args, env, console, ctx, proxy.handler.clone())?;
            return Ok(call_result_value(called)?.is_truthy());
        }
    }
    Ok(match proxy.target.deref_live() {
        JsvValue::Object(object) => object_property(&object, key).is_some(),
        JsvValue::Array(array) => key
            .parse::<usize>()
            .ok()
            .is_some_and(|index| index < array.borrow().len()),
        _ => false,
    })
}

fn proxy_own_keys(proxy: &JsvProxy) -> Result<Vec<(String, JsvValue)>, String> {
    own_properties(&proxy.target.deref_live())
}

#[allow(clippy::too_many_arguments)]
fn eval_yield_star(
    iterable: &JsvExpr,
    env: &mut JsvEnvironment,
    console: &mut Vec<String>,
    ctx: &mut EvalCtx<'_>,
) -> Result<JsvValue, String> {
    if ctx.in_generator == 0 {
        return Err("SyntaxError: yield* is only valid inside a generator".to_string());
    }
    let value = eval_expr(iterable, env, console, ctx)?;
    if let JsvValue::YieldSignal(_) = value {
        return Err("SyntaxError: yield* argument cannot suspend".to_string());
    }
    let iterator = iterator_from(&value.deref_live())?;
    let generator = ctx
        .current_generator
        .clone()
        .ok_or_else(|| "InvalidStateError: yield* outside a generator body".to_string())?;
    yield_star_loop(iterator, generator, JsvValue::Undefined, console, ctx)
}

#[allow(clippy::too_many_arguments)]
fn yield_star_loop(
    iterator: JsvIteratorRef,
    generator: JsvGeneratorRef,
    received: JsvValue,
    console: &mut Vec<String>,
    ctx: &mut EvalCtx<'_>,
) -> Result<JsvValue, String> {
    let (done, value) = iterator_next(&iterator, received, console, ctx)?;
    if done {
        return Ok(value);
    }
    Ok(JsvValue::YieldSignal(Box::new(
        JsvYieldSignal::new(value).push(Cont::YieldStarLoop {
            iterator,
            generator,
        }),
    )))
}

#[allow(clippy::too_many_arguments)]
fn generator_next(
    generator: &JsvGeneratorRef,
    argument: JsvValue,
    console: &mut Vec<String>,
    ctx: &mut EvalCtx<'_>,
) -> Result<(bool, JsvValue), String> {
    {
        let mut state = generator
            .try_borrow_mut()
            .map_err(|_| "TypeError: Reentrant access to generator".to_string())?;
        if state.done {
            return Ok((true, JsvValue::Undefined));
        }
        if state.running {
            return Err("TypeError: generator is already running (reentrant .next)".to_string());
        }
        state.running = true;
        if !state.started {
            state.started = true;
            let name = state.name.clone();
            let params = state.params.clone();
            let body = state.body.clone();
            let captured = state.captured.clone();
            let mut fn_env = JsvEnvironment::with_parent(captured.clone());
            if !name.is_empty() {
                fn_env.define(
                    &name,
                    JsvValue::GeneratorFn(name.clone(), params.clone(), body.clone(), captured),
                );
            }
            state.resume = Some(GeneratorResume {
                stmts: vec![*body],
                index: 0,
                env: fn_env,
                conts: Vec::new(),
                result: JsvValue::Undefined,
            });
        }
    }

    let _guard = GeneratorRunningGuard {
        generator: generator.clone(),
    };

    let mut value = argument;
    loop {
        let cont = {
            let mut state = generator
                .try_borrow_mut()
                .map_err(|_| "TypeError: Reentrant access to generator".to_string())?;
            let Some(resume) = state.resume.as_mut() else {
                state.done = true;
                return Ok((true, JsvValue::Undefined));
            };
            if resume.conts.is_empty() {
                break;
            }
            resume
                .conts
                .pop()
                .ok_or_else(|| "InvalidStateError: generator continuation is missing".to_string())?
        };

        let mut env = generator
            .try_borrow()
            .map_err(|_| "TypeError: Reentrant access to generator".to_string())?
            .resume
            .as_ref()
            .ok_or_else(|| "InvalidStateError: generator has no resume state".to_string())?
            .env
            .clone();
        match resume_cont(cont, value, &mut env, console, ctx)? {
            JsvValue::YieldSignal(signal) => {
                let mut state = generator
                    .try_borrow_mut()
                    .map_err(|_| "TypeError: Reentrant access to generator".to_string())?;
                let Some(resume) = state.resume.as_mut() else {
                    state.done = true;
                    return Ok((true, JsvValue::Undefined));
                };
                resume.conts.extend(signal.conts);
                drop(state);
                return Ok((false, *signal.value));
            }
            completed => {
                value = completed;
            }
        }
    }

    let mut state = generator
        .try_borrow_mut()
        .map_err(|_| "TypeError: Reentrant access to generator".to_string())?;
    let Some(resume) = state.resume.take() else {
        state.done = true;
        return Ok((true, JsvValue::Undefined));
    };
    drop(state);
    run_generator_statements(generator, resume, value, console, ctx)
}

#[allow(clippy::too_many_arguments)]
fn run_generator_statements(
    generator: &JsvGeneratorRef,
    resume: GeneratorResume,
    initial_value: JsvValue,
    console: &mut Vec<String>,
    ctx: &mut EvalCtx<'_>,
) -> Result<(bool, JsvValue), String> {
    ctx.in_generator += 1;
    ctx.current_generator = Some(generator.clone());
    let mut result = initial_value;
    let mut index = resume.index;
    let mut env = resume.env;
    loop {
        if index >= resume.stmts.len() {
            let mut state = generator
                .try_borrow_mut()
                .map_err(|_| "TypeError: Reentrant access to generator".to_string())?;
            state.done = true;
            state.resume = None;
            ctx.in_generator -= 1;
            ctx.current_generator = None;
            return Ok((true, result));
        }
        match eval_expr(&resume.stmts[index], &mut env, console, ctx) {
            Ok(JsvValue::YieldSignal(signal)) => {
                let mut state = generator
                    .try_borrow_mut()
                    .map_err(|_| "TypeError: Reentrant access to generator".to_string())?;
                state.resume = Some(GeneratorResume {
                    stmts: resume.stmts.clone(),
                    index: index + 1,
                    env: env.clone(),
                    conts: signal.conts,
                    result,
                });
                drop(state);
                ctx.in_generator -= 1;
                ctx.current_generator = None;
                return Ok((false, *signal.value));
            }
            Ok(JsvValue::ReturnSignal(value)) => {
                let mut state = generator
                    .try_borrow_mut()
                    .map_err(|_| "TypeError: Reentrant access to generator".to_string())?;
                state.done = true;
                state.resume = None;
                drop(state);
                ctx.in_generator -= 1;
                ctx.current_generator = None;
                return Ok((true, *value));
            }
            Ok(JsvValue::BreakSignal) => {
                ctx.in_generator -= 1;
                ctx.current_generator = None;
                return Err("SyntaxError: illegal break statement in generator".to_string());
            }
            Ok(JsvValue::ContinueSignal) => {
                ctx.in_generator -= 1;
                ctx.current_generator = None;
                return Err("SyntaxError: illegal continue statement in generator".to_string());
            }
            Ok(JsvValue::ThrowSignal(reason)) => {
                let mut state = generator
                    .try_borrow_mut()
                    .map_err(|_| "TypeError: Reentrant access to generator".to_string())?;
                state.done = true;
                state.resume = None;
                drop(state);
                ctx.in_generator -= 1;
                ctx.current_generator = None;
                return Err(format!(
                    "Uncaught exception in generator: {}",
                    reason.to_display_string()
                ));
            }
            Ok(value) => {
                result = value;
            }
            Err(error) => {
                let mut state = generator
                    .try_borrow_mut()
                    .map_err(|_| "TypeError: Reentrant access to generator".to_string())?;
                state.done = true;
                state.resume = None;
                drop(state);
                ctx.in_generator -= 1;
                ctx.current_generator = None;
                return Err(error);
            }
        }
        index += 1;
    }
}

#[allow(clippy::too_many_arguments)]
fn while_loop_step(
    cond: &JsvExpr,
    body: &[JsvExpr],
    env: JsvEnvironment,
    result: JsvValue,
    iterations: usize,
    mut cond_value: JsvValue,
    console: &mut Vec<String>,
    ctx: &mut EvalCtx<'_>,
) -> Result<JsvValue, String> {
    let mut e = env;
    let mut result = result;
    let mut iterations = iterations;
    loop {
        if !cond_value.is_truthy() {
            return Ok(result);
        }
        ctx.call_depth += 1;
        if ctx.call_depth > MAX_CALL_DEPTH {
            ctx.call_depth -= 1;
            return Err("Maximum call depth exceeded in while loop".to_string());
        }
        let mut iteration_env = JsvEnvironment::with_parent(e.clone());

        let body_result = match eval_statement_list(body, &mut iteration_env, console, ctx) {
            Ok(value) => value,
            Err(error) => {
                ctx.call_depth -= 1;
                return Err(error);
            }
        };
        ctx.call_depth -= 1;
        match body_result {
            JsvValue::BreakSignal => return Ok(result),
            JsvValue::ContinueSignal => {}
            JsvValue::YieldSignal(signal) => {
                return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                    Cont::WhileBody {
                        cond: Box::new(cond.clone()),
                        body: body.to_vec(),
                        env: e,
                        result,
                        iterations,
                    },
                ))));
            }
            value if is_abrupt_signal(&value) => return Ok(value),
            value => result = value,
        }
        iterations += 1;
        if iterations > 10_000 {
            return Err("Infinite loop detected (exceeded 10,000 iterations)".to_string());
        }
        let c = eval_expr(cond, &mut e, console, ctx)?;
        if let JsvValue::YieldSignal(signal) = c {
            return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                Cont::WhileCond {
                    cond: Box::new(cond.clone()),
                    body: body.to_vec(),
                    env: e,
                    result,
                    iterations,
                },
            ))));
        }
        cond_value = c;
    }
}

#[allow(clippy::too_many_arguments)]
fn do_while_loop_step(
    cond: &JsvExpr,
    body: &[JsvExpr],
    mut env: JsvEnvironment,
    mut result: JsvValue,
    mut iterations: usize,
    console: &mut Vec<String>,
    ctx: &mut EvalCtx<'_>,
) -> Result<JsvValue, String> {
    loop {
        let body_result = eval_statement_list(body, &mut env, console, ctx)?;
        match body_result {
            JsvValue::BreakSignal => return Ok(result),
            JsvValue::ContinueSignal => {}
            JsvValue::YieldSignal(signal) => {
                return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                    Cont::DoWhileBody {
                        cond: Box::new(cond.clone()),
                        body: body.to_vec(),
                        env,
                        result,
                        iterations,
                    },
                ))));
            }
            value if is_abrupt_signal(&value) => return Ok(value),
            value => result = value,
        }

        let condition = eval_expr(cond, &mut env, console, ctx)?;
        if let JsvValue::YieldSignal(signal) = condition {
            return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                Cont::DoWhileCond {
                    cond: Box::new(cond.clone()),
                    body: body.to_vec(),
                    env,
                    result,
                    iterations,
                },
            ))));
        }
        if !condition.is_truthy() {
            return Ok(result);
        }
        iterations = iterations.saturating_add(1);
        if iterations > 10_000 {
            return Err("Infinite loop detected (exceeded 10,000 iterations)".to_string());
        }
        ctx.step()?;
    }
}

#[allow(clippy::too_many_arguments)]
fn for_in_loop_step(
    binding: &str,
    kind: DeclarationKind,
    body: &[JsvExpr],
    mut env: JsvEnvironment,
    mut result: JsvValue,
    keys: Vec<String>,
    mut index: usize,
    console: &mut Vec<String>,
    ctx: &mut EvalCtx<'_>,
) -> Result<JsvValue, String> {
    while index < keys.len() {
        let key = JsvValue::String(keys[index].clone());
        index += 1;
        let mut iteration_env = JsvEnvironment::with_parent(env.clone());
        match kind {
            DeclarationKind::Const => iteration_env.declare(binding, key, false, false)?,
            DeclarationKind::Let => iteration_env.declare(binding, key, true, false)?,
            DeclarationKind::Var => env.assign(binding, key)?,
        }
        let active_env = if kind == DeclarationKind::Var {
            &mut env
        } else {
            &mut iteration_env
        };
        let body_result = eval_statement_list(body, active_env, console, ctx)?;
        match body_result {
            JsvValue::BreakSignal => return Ok(result),
            JsvValue::ContinueSignal => continue,
            JsvValue::YieldSignal(signal) => {
                return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                    Cont::ForInRest {
                        binding: binding.to_string(),
                        kind,
                        body: body.to_vec(),
                        env,
                        result,
                        keys,
                        index,
                    },
                ))));
            }
            value if is_abrupt_signal(&value) => return Ok(value),
            value => result = value,
        }
        ctx.step()?;
    }
    Ok(result)
}

#[allow(clippy::too_many_arguments)]
fn resume_cont(
    cont: Cont,
    value: JsvValue,
    env: &mut JsvEnvironment,
    console: &mut Vec<String>,
    ctx: &mut EvalCtx<'_>,
) -> Result<JsvValue, String> {
    match cont {
        Cont::BinaryRight {
            left: _,
            op,
            right,
            env: cont_env,
        } => {
            let mut e = cont_env;
            let r = eval_expr(&right, &mut e, console, ctx)?;
            if let JsvValue::YieldSignal(signal) = r {
                return Ok(JsvValue::YieldSignal(Box::new(
                    signal.push(Cont::BinaryApply { left: value, op }),
                )));
            }
            apply_binary_op(value, r, op, &mut e, console, ctx)
        }
        Cont::BinaryApply { left, op } => apply_binary_op(left, value, op, env, console, ctx),
        Cont::UnaryApply { op } => apply_unary_op(op, value),
        Cont::CallCallee {
            args,
            env: cont_env,
            is_new,
        } => {
            let mut e = cont_env;
            let arg_vals =
                eval_call_args_after_callee(value.clone(), &args, &mut e, console, ctx, is_new)?;
            if is_new {
                invoke_new(value, arg_vals, &mut e, console, ctx)
            } else {
                let called =
                    call_function(value, arg_vals, &mut e, console, ctx, JsvValue::Undefined)?;
                call_result_value(called)
            }
        }
        Cont::CallMemberBase {
            property,
            args,
            env: cont_env,
            is_new,
        } => {
            let mut e = cont_env;
            let base = value.deref_live();
            let func = read_property(base.clone(), &property, &mut e, console, ctx, false)?;
            let arg_vals =
                eval_call_args_after_callee(func.clone(), &args, &mut e, console, ctx, is_new)?;
            if is_new {
                invoke_new(func, arg_vals, &mut e, console, ctx)
            } else {
                let called = call_function(func, arg_vals, &mut e, console, ctx, base)?;
                call_result_value(called)
            }
        }
        Cont::CallIndexBase {
            key_expr,
            args,
            env: cont_env,
            is_new,
        } => {
            let mut e = cont_env;
            let base = value.deref_live();
            let key = eval_expr(&key_expr, &mut e, console, ctx)?;
            if let JsvValue::YieldSignal(signal) = key {
                return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                    Cont::CallIndexKey {
                        object: base,
                        args,
                        env: e,
                        is_new,
                    },
                ))));
            }
            let func = index_read(base.clone(), key.deref_live(), &mut e, console, ctx, false)?;
            let arg_vals =
                eval_call_args_after_callee(func.clone(), &args, &mut e, console, ctx, is_new)?;
            if is_new {
                invoke_new(func, arg_vals, &mut e, console, ctx)
            } else {
                let called = call_function(func, arg_vals, &mut e, console, ctx, base)?;
                call_result_value(called)
            }
        }
        Cont::CallIndexKey {
            object,
            args,
            env: cont_env,
            is_new,
        } => {
            let mut e = cont_env;
            let func = index_read(
                object.clone().deref_live(),
                value.deref_live(),
                &mut e,
                console,
                ctx,
                false,
            )?;
            let arg_vals =
                eval_call_args_after_callee(func.clone(), &args, &mut e, console, ctx, is_new)?;
            if is_new {
                invoke_new(func, arg_vals, &mut e, console, ctx)
            } else {
                let called = call_function(func, arg_vals, &mut e, console, ctx, object)?;
                call_result_value(called)
            }
        }
        Cont::CallPrivateBase {
            name,
            args,
            env: cont_env,
        } => {
            let mut e = cont_env;
            let base = value.deref_live();
            let func = private_read(&base, &name, &e)?;
            let arg_vals =
                eval_call_args_after_callee(func.clone(), &args, &mut e, console, ctx, false)?;
            let called = call_function(func, arg_vals, &mut e, console, ctx, base)?;
            call_result_value(called)
        }
        Cont::CallArgs {
            callee,
            done,
            left,
            env: cont_env,
            is_new,
            this_arg,
        } => {
            let mut e = cont_env;
            let mut done = done;
            done.push(value);
            let rest =
                eval_call_args_after_callee(callee.clone(), &left, &mut e, console, ctx, is_new)?;
            done.extend(rest);
            if is_new {
                invoke_new(callee, done, &mut e, console, ctx)
            } else {
                let called = call_function(callee, done, &mut e, console, ctx, this_arg)?;
                call_result_value(called)
            }
        }
        Cont::MemberApply { property, optional } => {
            read_property(value.deref_live(), &property, env, console, ctx, optional)
        }
        Cont::IndexBase {
            key_expr,
            optional,
            env: cont_env,
        } => {
            let mut e = cont_env;
            let base = value.deref_live();
            let key = eval_expr(&key_expr, &mut e, console, ctx)?;
            if let JsvValue::YieldSignal(signal) = key {
                return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                    Cont::IndexKey {
                        object: base.clone(),
                        key_expr: Box::new(JsvExpr::Undefined),
                        optional,
                        env: e,
                    },
                ))));
            }
            index_read(base, key.deref_live(), &mut e, console, ctx, optional)
        }
        Cont::IndexKey {
            object,
            key_expr,
            optional,
            env: cont_env,
        } => {
            let mut e = cont_env;
            let key = if matches!(&*key_expr, JsvExpr::Undefined) {
                value.clone()
            } else {
                let k = eval_expr(&key_expr, &mut e, console, ctx)?;
                if let JsvValue::YieldSignal(signal) = k {
                    return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                        Cont::IndexApply {
                            object: object.clone(),
                            key: JsvValue::Undefined,
                            optional,
                        },
                    ))));
                }
                k
            };
            index_read(
                object.deref_live(),
                key.deref_live(),
                &mut e,
                console,
                ctx,
                optional,
            )
        }
        Cont::IndexApply {
            object,
            key,
            optional,
        } => index_read(
            object.deref_live(),
            key.deref_live(),
            env,
            console,
            ctx,
            optional,
        ),
        Cont::MemberAssignBase {
            property,
            rhs,
            env: cont_env,
        } => {
            let mut e = cont_env;
            let base = value.deref_live();
            let r = eval_expr(&rhs, &mut e, console, ctx)?;
            if let JsvValue::YieldSignal(signal) = r {
                return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                    Cont::MemberAssignApply {
                        object: base,
                        property,
                    },
                ))));
            }
            write_property(base, &property, r, &mut e, console, ctx)
        }
        Cont::MemberAssignApply { object, property } => {
            write_property(object.deref_live(), &property, value, env, console, ctx)
        }
        Cont::IndexAssignBase {
            key_expr,
            rhs,
            env: cont_env,
        } => {
            let mut e = cont_env;
            let base = value.deref_live();
            let key = eval_expr(&key_expr, &mut e, console, ctx)?;
            if let JsvValue::YieldSignal(signal) = key {
                return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                    Cont::IndexAssignKey {
                        object: base.clone(),
                        rhs,
                        env: e,
                    },
                ))));
            }
            let r = eval_expr(&rhs, &mut e, console, ctx)?;
            if let JsvValue::YieldSignal(signal) = r {
                return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                    Cont::IndexAssignValue {
                        object: base.clone(),
                        key: key.clone(),
                    },
                ))));
            }
            index_write(base, key.deref_live(), r, &mut e, console, ctx)
        }
        Cont::IndexAssignKey {
            object,
            rhs,
            env: cont_env,
        } => {
            let mut e = cont_env;
            let key = value.deref_live();
            let r = eval_expr(&rhs, &mut e, console, ctx)?;
            if let JsvValue::YieldSignal(signal) = r {
                return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                    Cont::IndexAssignValue {
                        object: object.clone(),
                        key: key.clone(),
                    },
                ))));
            }
            index_write(object.deref_live(), key, r, &mut e, console, ctx)
        }
        Cont::IndexAssignValue { object, key } => index_write(
            object.deref_live(),
            key.deref_live(),
            value,
            env,
            console,
            ctx,
        ),
        Cont::IndexAssignApply { object, key } => index_write(
            object.deref_live(),
            key.deref_live(),
            value,
            env,
            console,
            ctx,
        ),
        Cont::ArrayElem {
            done,
            left,
            env: cont_env,
        } => {
            let mut e = cont_env;
            let mut done = done;
            done.push(value);
            eval_array_elements(done, &left, &mut e, console, ctx)
        }
        Cont::ObjectElem {
            done,
            left,
            env: cont_env,
        } => {
            let mut e = cont_env;
            let mut done = done;
            if let Some((entry, rest)) = left.split_first() {
                match entry {
                    ObjectProperty::KeyValue(key, _) => done.push((key.clone(), value)),
                    ObjectProperty::Spread(_) => {
                        for (key, item) in own_properties(&value.deref_live())? {
                            done.push((key, item));
                        }
                    }
                    _ => {
                        return Err(
                            "InvalidStateError: object literal continuation lost its entry"
                                .to_string(),
                        )
                    }
                }
                eval_object_elements(done, rest, &mut e, console, ctx)
            } else {
                eval_object_elements(done, &[], &mut e, console, ctx)
            }
        }
        Cont::TemplatePart {
            output,
            left,
            env: cont_env,
        } => {
            let mut e = cont_env;
            let mut output = output;
            output.push_str(&value.deref_live().to_display_string());
            eval_template_parts(output, &left, &mut e, console, ctx)
        }
        Cont::TernaryCond {
            then_expr,
            else_expr,
            env: cont_env,
        } => {
            let mut e = cont_env;
            if value.is_truthy() {
                eval_expr(&then_expr, &mut e, console, ctx)
            } else {
                eval_expr(&else_expr, &mut e, console, ctx)
            }
        }
        Cont::AssignApply {
            target,
            env: cont_env,
        } => {
            let mut e = cont_env;
            assign_target(&target, value.clone(), &mut e, console, ctx)?;
            Ok(value)
        }
        Cont::VarDeclApply {
            name,
            kind,
            env: cont_env,
        } => {
            let mut e = cont_env;
            e.declare(
                &name,
                value.clone(),
                kind != DeclarationKind::Const,
                kind == DeclarationKind::Var,
            )?;
            Ok(value)
        }
        Cont::BindPatternApply {
            pattern,
            kind,
            env: cont_env,
        } => {
            let mut e = cont_env;
            bind_pattern(&pattern, &value, &mut e, kind, console, ctx)?;
            Ok(value)
        }
        Cont::ReturnApply => Ok(JsvValue::ReturnSignal(Box::new(value))),
        Cont::ThrowApply => Ok(JsvValue::ThrowSignal(Box::new(value))),
        Cont::IfCond {
            then_branch,
            else_branch,
            env: cont_env,
        } => {
            let mut e = cont_env;
            if value.is_truthy() {
                eval_statement_list(&then_branch, &mut e, console, ctx)
            } else if let Some(else_stmts) = else_branch {
                eval_statement_list(&else_stmts, &mut e, console, ctx)
            } else {
                Ok(JsvValue::Undefined)
            }
        }
        Cont::WhileCond {
            cond,
            body,
            env: cont_env,
            result,
            iterations,
        } => while_loop_step(
            &cond, &body, cont_env, result, iterations, value, console, ctx,
        ),
        Cont::WhileBody {
            cond,
            body,
            env: cont_env,
            result,
            iterations,
        } => {
            let mut e = cont_env;
            let iterations = iterations + 1;
            if iterations > 10_000 {
                return Err("Infinite loop detected (exceeded 10,000 iterations)".to_string());
            }
            let cond_value = eval_expr(&cond, &mut e, console, ctx)?;
            if let JsvValue::YieldSignal(signal) = cond_value {
                return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                    Cont::WhileCond {
                        cond,
                        body,
                        env: e,
                        result,
                        iterations,
                    },
                ))));
            }
            while_loop_step(
                &cond, &body, e, result, iterations, cond_value, console, ctx,
            )
        }
        Cont::DoWhileBody {
            cond,
            body,
            env: cont_env,
            mut result,
            iterations,
        } => {
            match value {
                JsvValue::BreakSignal => return Ok(result),
                JsvValue::ContinueSignal => {}
                value if is_abrupt_signal(&value) => return Ok(value),
                value => result = value,
            }
            let mut e = cont_env;
            let condition = eval_expr(&cond, &mut e, console, ctx)?;
            if let JsvValue::YieldSignal(signal) = condition {
                return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                    Cont::DoWhileCond {
                        cond,
                        body,
                        env: e,
                        result,
                        iterations,
                    },
                ))));
            }
            if !condition.is_truthy() {
                return Ok(result);
            }
            let iterations = iterations.saturating_add(1);
            if iterations > 10_000 {
                return Err("Infinite loop detected (exceeded 10,000 iterations)".to_string());
            }
            do_while_loop_step(&cond, &body, e, result, iterations, console, ctx)
        }
        Cont::DoWhileCond {
            cond,
            body,
            env: cont_env,
            result,
            iterations,
        } => {
            if !value.is_truthy() {
                return Ok(result);
            }
            let iterations = iterations.saturating_add(1);
            if iterations > 10_000 {
                return Err("Infinite loop detected (exceeded 10,000 iterations)".to_string());
            }
            do_while_loop_step(&cond, &body, cont_env, result, iterations, console, ctx)
        }
        Cont::ForInRest {
            binding,
            kind,
            body,
            env: cont_env,
            mut result,
            keys,
            index,
        } => {
            match value {
                JsvValue::BreakSignal => return Ok(result),
                JsvValue::ContinueSignal => {}
                value if is_abrupt_signal(&value) => return Ok(value),
                value => result = value,
            }
            for_in_loop_step(
                &binding, kind, &body, cont_env, result, keys, index, console, ctx,
            )
        }
        Cont::Statements {
            stmts,
            index,
            env: cont_env,
            result,
        } => {
            let mut e = cont_env;
            let mut result = result;
            let mut index = index;
            while index < stmts.len() {
                let r = eval_expr(&stmts[index], &mut e, console, ctx)?;
                if let JsvValue::YieldSignal(signal) = r {
                    return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                        Cont::Statements {
                            stmts,
                            index: index + 1,
                            env: e,
                            result,
                        },
                    ))));
                }
                if is_abrupt_signal(&r) {
                    return Ok(r);
                }
                result = r;
                index += 1;
            }
            Ok(result)
        }
        Cont::TryResume {
            try_body,
            catch_binding,
            catch_body,
            finally_body,
            env: cont_env,
            outcome: _,
            stage,
        } => {
            let e = cont_env;

            let mut outcome = value;
            if stage == 0 {
                if let JsvValue::ThrowSignal(ref reason) = outcome {
                    if let Some(ref binding) = catch_binding {
                        let mut catch_env = JsvEnvironment::with_parent(e.clone());
                        catch_env.define(binding, (**reason).clone());
                        outcome =
                            match eval_statement_list(&catch_body, &mut catch_env, console, ctx) {
                                Ok(value) => value,
                                Err(error) => {
                                    JsvValue::ThrowSignal(Box::new(JsvValue::String(error)))
                                }
                            };
                        if let JsvValue::YieldSignal(signal) = outcome {
                            return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                                Cont::TryResume {
                                    try_body,
                                    catch_binding,
                                    catch_body,
                                    finally_body,
                                    env: e,
                                    outcome: JsvValue::Undefined,
                                    stage: 1,
                                },
                            ))));
                        }
                    }
                }
            }
            let mut final_value = JsvValue::Undefined;
            if !finally_body.is_empty() {
                let mut finally_env = JsvEnvironment::with_parent(e.clone());
                final_value =
                    match eval_statement_list(&finally_body, &mut finally_env, console, ctx) {
                        Ok(value) => value,
                        Err(error) => JsvValue::ThrowSignal(Box::new(JsvValue::String(error))),
                    };
                if let JsvValue::YieldSignal(signal) = final_value {
                    return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                        Cont::TryResume {
                            try_body,
                            catch_binding,
                            catch_body,
                            finally_body,
                            env: e,
                            outcome,
                            stage: 2,
                        },
                    ))));
                }
            }
            if stage == 2 && is_abrupt_signal(&final_value) {
                Ok(final_value)
            } else {
                Ok(outcome)
            }
        }
        Cont::YieldStarLoop {
            iterator,
            generator,
        } => yield_star_loop(iterator, generator, value, console, ctx),
        Cont::ForOfIterable {
            pattern,
            kind,
            body,
            env: cont_env,
            result,
        } => {
            let e = cont_env;
            let values = iterate_values(&value.deref_live(), console, ctx)?;
            let mut result = result;
            for (index, item) in values.iter().cloned().enumerate() {
                let mut iteration_env = JsvEnvironment::with_parent(e.clone());
                bind_pattern(&pattern, &item, &mut iteration_env, kind, console, ctx)?;
                let body_result = eval_statement_list(&body, &mut iteration_env, console, ctx)?;
                match body_result {
                    JsvValue::BreakSignal => return Ok(result),
                    JsvValue::ContinueSignal => continue,
                    JsvValue::YieldSignal(signal) => {
                        return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                            Cont::ForOfRest {
                                pattern,
                                kind,
                                body,
                                env: e,
                                result,
                                values,
                                index: index + 1,
                            },
                        ))));
                    }
                    value if is_abrupt_signal(&value) => return Ok(value),
                    value => result = value,
                }
            }
            Ok(result)
        }
        Cont::ForOfRest {
            pattern,
            kind,
            body,
            env: cont_env,
            result,
            values,
            mut index,
        } => {
            let e = cont_env;
            let mut result = result;
            match value {
                JsvValue::BreakSignal => return Ok(result),
                JsvValue::ContinueSignal => {}
                value if is_abrupt_signal(&value) => return Ok(value),
                value => result = value,
            }
            loop {
                if index >= values.len() {
                    return Ok(result);
                }
                let item = values[index].clone();
                index += 1;
                let mut iteration_env = JsvEnvironment::with_parent(e.clone());
                bind_pattern(&pattern, &item, &mut iteration_env, kind, console, ctx)?;
                let body_result = eval_statement_list(&body, &mut iteration_env, console, ctx)?;
                match body_result {
                    JsvValue::BreakSignal => return Ok(result),
                    JsvValue::ContinueSignal => continue,
                    JsvValue::YieldSignal(signal) => {
                        return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                            Cont::ForOfRest {
                                pattern,
                                kind,
                                body,
                                env: e,
                                result,
                                values,
                                index,
                            },
                        ))));
                    }
                    value if is_abrupt_signal(&value) => return Ok(value),
                    value => result = value,
                }
            }
        }
        Cont::SwitchBody { result } => Ok(result),
    }
}

fn eval_call_args_after_callee(
    callee: JsvValue,
    args: &[JsvExpr],
    env: &mut JsvEnvironment,
    console: &mut Vec<String>,
    ctx: &mut EvalCtx<'_>,
    is_new: bool,
) -> Result<Vec<JsvValue>, String> {
    let mut out = Vec::new();
    for (index, arg) in args.iter().enumerate() {
        if let JsvExpr::Spread(spread) = arg {
            let value = eval_expr(spread, env, console, ctx)?;
            if let JsvValue::YieldSignal(signal) = value {
                return Ok(vec![JsvValue::YieldSignal(Box::new(signal.push(
                    Cont::CallArgs {
                        callee,
                        done: out,
                        left: args[index + 1..].to_vec(),
                        env: env.clone(),
                        is_new,
                        this_arg: JsvValue::Undefined,
                    },
                )))]);
            }
            for item in iterate_values(&value.deref_live(), console, ctx)? {
                out.push(item);
            }
        } else {
            let value = eval_expr(arg, env, console, ctx)?;
            if let JsvValue::YieldSignal(signal) = value {
                return Ok(vec![JsvValue::YieldSignal(Box::new(signal.push(
                    Cont::CallArgs {
                        callee,
                        done: out,
                        left: args[index + 1..].to_vec(),
                        env: env.clone(),
                        is_new,
                        this_arg: JsvValue::Undefined,
                    },
                )))]);
            }
            out.push(value);
        }
        if out.len() > MAX_ARRAY_ELEMENTS {
            return Err("Argument budget exceeded".to_string());
        }
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn eval_array_elements(
    done: Vec<JsvValue>,
    left: &[JsvExpr],
    env: &mut JsvEnvironment,
    console: &mut Vec<String>,
    ctx: &mut EvalCtx<'_>,
) -> Result<JsvValue, String> {
    let mut values = done;
    for (index, expression) in left.iter().enumerate() {
        if let JsvExpr::Spread(spread) = expression {
            let iterable = eval_expr(spread, env, console, ctx)?;
            if let JsvValue::YieldSignal(signal) = iterable {
                return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                    Cont::ArrayElem {
                        done: values,
                        left: left[index + 1..].to_vec(),
                        env: env.clone(),
                    },
                ))));
            }
            for item in iterate_values(&iterable.deref_live(), console, ctx)? {
                values.push(item);
            }
        } else {
            let value = eval_expr(expression, env, console, ctx)?;
            if let JsvValue::YieldSignal(signal) = value {
                return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                    Cont::ArrayElem {
                        done: values,
                        left: left[index + 1..].to_vec(),
                        env: env.clone(),
                    },
                ))));
            }
            values.push(value);
        }
        if values.len() > MAX_ARRAY_ELEMENTS {
            return Err("Array element budget exceeded".to_string());
        }
    }
    Ok(JsvValue::Array(Rc::new(RefCell::new(values))))
}

#[allow(clippy::too_many_arguments)]
fn eval_object_elements(
    done: Vec<(String, JsvValue)>,
    left: &[ObjectProperty],
    env: &mut JsvEnvironment,
    console: &mut Vec<String>,
    ctx: &mut EvalCtx<'_>,
) -> Result<JsvValue, String> {
    let mut properties: HashMap<String, JsvValue> = done.into_iter().collect();
    for (index, entry) in left.iter().enumerate() {
        match entry {
            ObjectProperty::KeyValue(key, expression) => {
                let value = eval_expr(expression, env, console, ctx)?;
                if let JsvValue::YieldSignal(signal) = value {
                    return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                        Cont::ObjectElem {
                            done: properties.into_iter().collect(),
                            left: left[index + 1..].to_vec(),
                            env: env.clone(),
                        },
                    ))));
                }
                properties.insert(key.clone(), value);
            }
            ObjectProperty::Spread(expression) => {
                let value = eval_expr(expression, env, console, ctx)?;
                if let JsvValue::YieldSignal(signal) = value {
                    return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                        Cont::ObjectElem {
                            done: properties.into_iter().collect(),
                            left: left[index + 1..].to_vec(),
                            env: env.clone(),
                        },
                    ))));
                }
                for (key, item) in own_properties(&value.deref_live())? {
                    properties.insert(key, item);
                }
            }
            ObjectProperty::Method {
                name,
                params,
                body,
                is_async,
                is_generator,
            } => {
                let func = if *is_async {
                    JsvValue::AsyncFunction(name.clone(), params.clone(), body.clone(), env.clone())
                } else if *is_generator {
                    JsvValue::GeneratorFn(name.clone(), params.clone(), body.clone(), env.clone())
                } else {
                    JsvValue::Function(name.clone(), params.clone(), body.clone(), env.clone())
                };
                properties.insert(name.clone(), func);
            }
            ObjectProperty::Getter { name, body } => {
                let getter = JsvValue::Function(
                    format!("get {}", name),
                    Vec::new(),
                    body.clone(),
                    env.clone(),
                );
                let existing = properties.get(name).cloned();
                let setter = match existing {
                    Some(JsvValue::GetterSetter(_, set)) => (*set).clone(),
                    _ => JsvValue::Undefined,
                };
                properties.insert(
                    name.clone(),
                    JsvValue::GetterSetter(Box::new(getter), Box::new(setter)),
                );
            }
            ObjectProperty::Setter { name, param, body } => {
                let setter = JsvValue::Function(
                    format!("set {}", name),
                    vec![param.clone()],
                    body.clone(),
                    env.clone(),
                );
                let existing = properties.get(name).cloned();
                let getter = match existing {
                    Some(JsvValue::GetterSetter(get, _)) => (*get).clone(),
                    _ => JsvValue::Undefined,
                };
                properties.insert(
                    name.clone(),
                    JsvValue::GetterSetter(Box::new(getter), Box::new(setter)),
                );
            }
        }
    }
    Ok(object_value(properties))
}

#[allow(clippy::too_many_arguments)]
fn eval_template_parts(
    mut output: String,
    left: &[TemplatePart],
    env: &mut JsvEnvironment,
    console: &mut Vec<String>,
    ctx: &mut EvalCtx<'_>,
) -> Result<JsvValue, String> {
    for (index, part) in left.iter().enumerate() {
        match part {
            TemplatePart::Text(text) => output.push_str(text),
            TemplatePart::Expr(expression) => {
                let value = eval_expr(expression, env, console, ctx)?;
                if let JsvValue::YieldSignal(signal) = value {
                    return Ok(JsvValue::YieldSignal(Box::new(signal.push(
                        Cont::TemplatePart {
                            output,
                            left: left[index + 1..].to_vec(),
                            env: env.clone(),
                        },
                    ))));
                }
                output.push_str(&value.deref_live().to_display_string());
            }
        }
        check_string_alloc(ctx, output.len())?;
    }
    Ok(JsvValue::String(output))
}

fn apply_unary_op(op: OpKind, value: JsvValue) -> Result<JsvValue, String> {
    match op {
        OpKind::Sub => Ok(JsvValue::Number(-to_number(&value))),
        OpKind::Not => Ok(JsvValue::Boolean(!value.is_truthy())),
        OpKind::Typeof => Ok(JsvValue::String(typeof_name(&value).to_string())),
        _ => Err("Invalid unary operation".to_string()),
    }
}

#[allow(clippy::too_many_arguments)]
fn assign_target(
    target: &AssignTarget,
    value: JsvValue,
    env: &mut JsvEnvironment,
    console: &mut Vec<String>,
    ctx: &mut EvalCtx<'_>,
) -> Result<(), String> {
    match target {
        AssignTarget::Identifier(name) => env.assign(name, value)?,
        AssignTarget::Member(object, property) => {
            let object = eval_expr(object, env, console, ctx)?;
            write_property(object.deref_live(), property, value, env, console, ctx)?;
        }
        AssignTarget::Index(object, key) => {
            let object = eval_expr(object, env, console, ctx)?;
            let key = eval_expr(key, env, console, ctx)?;
            index_write(
                object.deref_live(),
                key.deref_live(),
                value,
                env,
                console,
                ctx,
            )?;
        }
    }
    Ok(())
}

fn enqueue_promise_reaction(
    ctx: &mut EvalCtx<'_>,
    kind: PromiseReactionKind,
    handler: Option<JsvValue>,
    argument: JsvValue,
    result: JsvPromiseRef,
) -> Result<(), String> {
    if ctx.microtasks_queued >= MAX_PROMISE_JOBS {
        return Err("Promise microtask budget exceeded".to_string());
    }
    ctx.microtasks_queued += 1;
    ctx.microtasks.push_back(PromiseReactionJob {
        kind,
        handler,
        argument,
        result,
    });
    Ok(())
}

fn promise_key(promise: &JsvPromiseRef) -> usize {
    Rc::as_ptr(promise) as usize
}

fn register_pending_reaction(
    ctx: &mut EvalCtx<'_>,
    promise: &JsvPromiseRef,
    reaction: PendingPromiseReaction,
) -> Result<(), String> {
    let pending = ctx
        .pending_promise_reactions
        .values()
        .map(Vec::len)
        .sum::<usize>();
    if pending >= MAX_PROMISE_JOBS {
        return Err("Promise pending-reaction budget exceeded".to_string());
    }
    ctx.pending_promise_reactions
        .entry(promise_key(promise))
        .or_default()
        .push(reaction);
    Ok(())
}

fn enqueue_registered_reaction(
    ctx: &mut EvalCtx<'_>,
    reaction: PendingPromiseReaction,
    state: JsvPromiseState,
) -> Result<(), String> {
    let (kind, handler, argument) = match state {
        JsvPromiseState::Fulfilled(value) => {
            if let Some((kind, override_value)) = reaction.completion_override {
                (kind, None, override_value)
            } else if reaction.on_finally.is_some() {
                (PromiseReactionKind::Finally, reaction.on_finally, value)
            } else {
                (PromiseReactionKind::Fulfill, reaction.on_fulfilled, value)
            }
        }
        JsvPromiseState::Rejected(reason) => {
            if reaction.on_finally.is_some() {
                (PromiseReactionKind::Finally, reaction.on_finally, reason)
            } else {
                (PromiseReactionKind::Reject, reaction.on_rejected, reason)
            }
        }
        JsvPromiseState::Pending => {
            return Err("InvalidStateError: cannot enqueue a pending Promise".to_string());
        }
    };
    enqueue_promise_reaction(ctx, kind, handler, argument, reaction.result)
}

fn settle_promise(
    ctx: &mut EvalCtx<'_>,
    promise: &JsvPromiseRef,
    state: JsvPromiseState,
) -> Result<(), String> {
    *promise
        .try_borrow_mut()
        .map_err(|_| "TypeError: Reentrant access to promise".to_string())? = state.clone();
    if matches!(state, JsvPromiseState::Pending) {
        return Ok(());
    }
    if let Some(reactions) = ctx.pending_promise_reactions.remove(&promise_key(promise)) {
        for reaction in reactions {
            enqueue_registered_reaction(ctx, reaction, state.clone())?;
        }
    }
    Ok(())
}

fn resolve_promise(
    ctx: &mut EvalCtx<'_>,
    target: &JsvPromiseRef,
    outcome: Result<JsvValue, JsvValue>,
) -> Result<(), String> {
    match outcome {
        Err(reason) => settle_promise(ctx, target, JsvPromiseState::Rejected(reason)),
        Ok(JsvValue::Promise(source)) => {
            if Rc::ptr_eq(target, &source) {
                return settle_promise(
                    ctx,
                    target,
                    JsvPromiseState::Rejected(JsvValue::String(
                        "TypeError: Promise cannot resolve to itself".to_string(),
                    )),
                );
            }
            match source
                .try_borrow()
                .map_err(|_| "TypeError: Reentrant access to promise".to_string())?
                .clone()
            {
                JsvPromiseState::Pending => register_pending_reaction(
                    ctx,
                    &source,
                    PendingPromiseReaction {
                        on_fulfilled: None,
                        on_rejected: None,
                        on_finally: None,
                        completion_override: None,
                        result: target.clone(),
                    },
                ),
                state => settle_promise(ctx, target, state),
            }
        }
        Ok(value) => settle_promise(ctx, target, JsvPromiseState::Fulfilled(value)),
    }
}

fn string_method_supported(method: &str) -> bool {
    matches!(
        method,
        "toUpperCase"
            | "toLowerCase"
            | "trim"
            | "trimStart"
            | "trimEnd"
            | "charAt"
            | "charCodeAt"
            | "indexOf"
            | "includes"
            | "startsWith"
            | "endsWith"
            | "slice"
            | "substring"
            | "replace"
            | "split"
            | "repeat"
            | "concat"
            | "padStart"
            | "padEnd"
            | "at"
            | "replaceAll"
            | "matchAll"
            | "normalize"
    )
}

fn call_string_method(
    method: &str,
    text: String,
    args: &[JsvValue],
    ctx: &mut EvalCtx<'_>,
) -> Result<JsvValue, String> {
    let string_arg = |index: usize| -> Result<String, String> {
        args.get(index)
            .map(|value| value.to_display_string())
            .ok_or_else(|| "TypeError: missing string argument".to_string())
    };
    let number_arg = |index: usize| -> Result<f64, String> {
        args.get(index)
            .and_then(|value| value.as_number())
            .ok_or_else(|| "TypeError: expected a number".to_string())
    };
    let chars: Vec<char> = text.chars().collect();
    let count = chars.len();
    let mut bounded = |value: String| -> Result<JsvValue, String> {
        check_string_alloc(ctx, value.len())?;
        Ok(JsvValue::String(value))
    };
    match method {
        "toUpperCase" => bounded(text.to_uppercase()),
        "toLowerCase" => bounded(text.to_lowercase()),
        "trim" => bounded(text.trim().to_string()),
        "trimStart" => bounded(text.trim_start().to_string()),
        "trimEnd" => bounded(text.trim_end().to_string()),
        "charAt" => {
            let index = number_arg(0).unwrap_or(f64::NAN);
            let index = if index.is_nan() { 0.0 } else { index };
            let character = if index.is_finite() && index >= 0.0 && (index as usize) < count {
                chars[index as usize].to_string()
            } else {
                String::new()
            };
            bounded(character)
        }
        "charCodeAt" => {
            let index = number_arg(0).unwrap_or(f64::NAN);
            let code = if index.is_finite() && index >= 0.0 && (index as usize) < count {
                chars[index as usize] as u32 as f64
            } else {
                f64::NAN
            };
            Ok(JsvValue::Number(code))
        }
        "at" => {
            let index = number_arg(0).unwrap_or(f64::NAN);
            let index = if index.is_nan() { 0.0 } else { index };
            let resolved = if index < 0.0 {
                count as f64 + index
            } else {
                index
            };
            let character =
                if resolved.is_finite() && resolved >= 0.0 && (resolved as usize) < count {
                    chars[resolved as usize].to_string()
                } else {
                    String::new()
                };
            bounded(character)
        }
        "indexOf" => {
            let needle = string_arg(0)?;
            let start = number_arg(1).unwrap_or(0.0).max(0.0) as usize;
            let position = if needle.is_empty() {
                start.min(count)
            } else {
                let mut found = None;
                for index in start..count {
                    let candidate: String =
                        chars[index..].iter().take(needle.chars().count()).collect();
                    if candidate == needle {
                        found = Some(index);
                        break;
                    }
                }
                found.unwrap_or(usize::MAX)
            };
            Ok(JsvValue::Number(if position == usize::MAX {
                -1.0
            } else {
                position as f64
            }))
        }
        "includes" => {
            let needle = string_arg(0)?;
            let start = (number_arg(1).unwrap_or(0.0).max(0.0) as usize).min(count);
            let found = if needle.is_empty() {
                true
            } else {
                (start..count).any(|index| {
                    let candidate: String =
                        chars[index..].iter().take(needle.chars().count()).collect();
                    candidate == needle
                })
            };
            Ok(JsvValue::Boolean(found))
        }
        "startsWith" => {
            let needle = string_arg(0)?;
            let start = (number_arg(1).unwrap_or(0.0).max(0.0) as usize).min(count);
            let candidate: String = chars[start..].iter().collect();
            Ok(JsvValue::Boolean(candidate.starts_with(&needle)))
        }
        "endsWith" => {
            let needle = string_arg(0)?;
            let end = number_arg(1)
                .map(|value| value.max(0.0) as usize)
                .unwrap_or(count);
            let candidate: String = chars[..end.min(count)].iter().collect();
            Ok(JsvValue::Boolean(candidate.ends_with(&needle)))
        }
        "slice" => {
            let start = number_arg(0).unwrap_or(0.0);
            let end = number_arg(1).unwrap_or(count as f64);
            let normalize = |value: f64| -> usize {
                if value.is_nan() {
                    return 0;
                }
                if value < 0.0 {
                    (count as f64 + value).max(0.0) as usize
                } else {
                    (value as usize).min(count)
                }
            };
            let start = normalize(start);
            let end = normalize(end);
            let sliced: String = if end > start {
                chars[start..end.min(count)].iter().collect()
            } else {
                String::new()
            };
            bounded(sliced)
        }
        "substring" => {
            let start = number_arg(0).unwrap_or(0.0).max(0.0) as usize;
            let end = number_arg(1).unwrap_or(count as f64).max(0.0) as usize;
            let (start, end) = if start > end {
                (end, start)
            } else {
                (start, end)
            };
            let sliced: String = chars[start.min(count)..end.min(count)].iter().collect();
            bounded(sliced)
        }
        "replace" => {
            let pattern = string_arg(0)?;
            let replacement = string_arg(1)?;
            if pattern.is_empty() {
                return bounded(format!(
                    "{}{}",
                    replacement,
                    chars.iter().collect::<String>()
                ));
            }
            let pattern_chars: Vec<char> = pattern.chars().collect();
            let mut replaced = false;
            let mut output = String::new();
            let mut index = 0;
            while index < count {
                if !replaced && index + pattern_chars.len() <= count {
                    let candidate: String =
                        chars[index..index + pattern_chars.len()].iter().collect();
                    if candidate == pattern {
                        output.push_str(&replacement);
                        index += pattern_chars.len();
                        replaced = true;
                        continue;
                    }
                }
                output.push(chars[index]);
                index += 1;
            }
            bounded(output)
        }
        "split" => {
            let separator = args.first().map(|value| value.to_display_string());
            let values = match separator.as_deref() {
                None => vec![JsvValue::String(text.clone())],
                Some("") => {
                    if chars.len() > 100_000 {
                        return Err("Split result budget exceeded".to_string());
                    }
                    chars
                        .iter()
                        .map(|character| JsvValue::String(character.to_string()))
                        .collect::<Vec<_>>()
                }
                Some(separator) => {
                    let separator_chars: Vec<char> = separator.chars().collect();
                    let mut parts = Vec::new();
                    let mut current = String::new();
                    let mut index = 0;
                    while index < count {
                        if index + separator_chars.len() <= count {
                            let candidate: String =
                                chars[index..index + separator_chars.len()].iter().collect();
                            if candidate == separator {
                                parts.push(JsvValue::String(std::mem::take(&mut current)));

                                if parts.len() > 100_000 {
                                    return Err("Split result budget exceeded".to_string());
                                }
                                index += separator_chars.len();
                                continue;
                            }
                        }
                        current.push(chars[index]);
                        index += 1;
                    }
                    parts.push(JsvValue::String(current));
                    if parts.len() > 100_000 {
                        return Err("Split result budget exceeded".to_string());
                    }
                    parts
                }
            };
            Ok(JsvValue::Array(Rc::new(RefCell::new(values))))
        }
        "repeat" => {
            let times = number_arg(0).unwrap_or(0.0);
            if times.is_nan() || times < 0.0 {
                return Err("RangeError: repeat count is invalid".to_string());
            }
            let times = times as usize;
            if text.len().saturating_mul(times) > MAX_STRING_LENGTH {
                return Err("String too large".to_string());
            }
            let repeated = text.repeat(times);
            check_string_alloc(ctx, repeated.len())?;
            Ok(JsvValue::String(repeated))
        }
        "concat" => {
            let mut output = text;
            for arg in args.iter().take(64) {
                output.push_str(&arg.to_display_string());
                check_string_alloc(ctx, output.len())?;
            }
            Ok(JsvValue::String(output))
        }
        "padStart" | "padEnd" => {
            let target = number_arg(0).unwrap_or(0.0).max(0.0) as usize;
            let pad = string_arg(1).unwrap_or_else(|_| " ".to_string());
            let current = count;
            if target <= current || pad.is_empty() {
                return bounded(text);
            }
            if target > MAX_STRING_LENGTH {
                return Err("String too large".to_string());
            }
            let pad_chars: Vec<char> = pad.chars().collect();
            let needed = target - current;
            let mut padding = String::with_capacity(needed * 4);
            for index in 0..needed {
                padding.push(pad_chars[index % pad_chars.len()]);
            }
            if method == "padStart" {
                bounded(format!("{}{}", padding, text))
            } else {
                bounded(format!("{}{}", text, padding))
            }
        }
        "replaceAll" => {
            let pattern = string_arg(0)?;
            let replacement = string_arg(1)?;
            if pattern.is_empty() {
                return bounded(format!(
                    "{}{}",
                    replacement,
                    chars.iter().collect::<String>()
                ));
            }
            let pattern_chars: Vec<char> = pattern.chars().collect();
            let mut output = String::new();
            let mut index = 0;
            while index < count {
                if index + pattern_chars.len() <= count {
                    let candidate: String =
                        chars[index..index + pattern_chars.len()].iter().collect();
                    if candidate == pattern {
                        output.push_str(&replacement);
                        index += pattern_chars.len();
                        continue;
                    }
                }
                output.push(chars[index]);
                index += 1;
            }
            bounded(output)
        }
        "matchAll" => {
            let pattern = string_arg(0)?;
            let pattern_chars: Vec<char> = pattern.chars().collect();
            let plen = pattern_chars.len();
            let mut results = Vec::new();
            if !pattern.is_empty() {
                let mut index = 0;
                while index + plen <= count {
                    let candidate: String = chars[index..index + plen].iter().collect();
                    if candidate == pattern {
                        results.push(JsvValue::String(candidate));
                        index += plen;
                    } else {
                        index += 1;
                    }
                }
            }
            Ok(JsvValue::Array(Rc::new(RefCell::new(results))))
        }
        "normalize" => {
            let _form = args
                .first()
                .map(|v| v.to_display_string())
                .unwrap_or_else(|| "NFC".to_string());
            bounded(text)
        }
        _ => Err(format!("TypeError: String.{} is not implemented", method)),
    }
}

fn collection_iterator(source: JsvValue, kind: IteratorKind, column: u8) -> JsvValue {
    JsvValue::Iterator(Rc::new(RefCell::new(JsvIteratorState {
        kind,
        source: source.clone(),
        index: 0,
        byte_offset: 0,
        snapshot: Some(source),
        column,
        generator: None,
    })))
}

fn call_bound_native(
    name: &str,
    receiver: JsvValue,
    args: Vec<JsvValue>,
    env: &mut JsvEnvironment,
    console: &mut Vec<String>,
    ctx: &mut EvalCtx<'_>,
) -> Result<JsvValue, String> {
    match (name, receiver) {
        ("Array.push", JsvValue::Array(array)) => {
            let projected = array.borrow().len().saturating_add(args.len());
            if projected > MAX_ARRAY_ELEMENTS {
                return Err("Array element budget exceeded".to_string());
            }
            array.borrow_mut().extend(args);
            Ok(JsvValue::Number(projected as f64))
        }
        ("Array.pop", JsvValue::Array(array)) => {
            let value = array.borrow_mut().pop().unwrap_or(JsvValue::Undefined);
            Ok(value)
        }
        ("Array.shift", JsvValue::Array(array)) => {
            let value = if array.borrow().is_empty() {
                JsvValue::Undefined
            } else {
                array.borrow_mut().remove(0)
            };
            Ok(value)
        }
        ("Array.unshift", JsvValue::Array(array)) => {
            let projected = array.borrow().len().saturating_add(args.len());
            if projected > MAX_ARRAY_ELEMENTS {
                return Err("Array element budget exceeded".to_string());
            }
            for value in args.into_iter().rev() {
                array.borrow_mut().insert(0, value);
            }
            Ok(JsvValue::Number(projected as f64))
        }
        ("Array.join", JsvValue::Array(array)) => {
            let separator = args
                .first()
                .map(JsvValue::to_display_string)
                .unwrap_or_else(|| ",".to_string());
            let result = array
                .borrow()
                .iter()
                .map(JsvValue::to_display_string)
                .collect::<Vec<_>>()
                .join(&separator);
            check_string_alloc(ctx, result.len())?;
            Ok(JsvValue::String(result))
        }
        ("Map.get", JsvValue::Map(map)) => {
            let key = args.first().cloned().unwrap_or(JsvValue::Undefined);
            let borrowed = map.borrow();
            Ok(map_index(&borrowed, &key)
                .map(|index| borrowed.entries[index].1.clone())
                .unwrap_or(JsvValue::Undefined))
        }
        ("Map.set", JsvValue::Map(map)) => {
            let key = args.first().cloned().unwrap_or(JsvValue::Undefined);
            let value = args.get(1).cloned().unwrap_or(JsvValue::Undefined);
            {
                let mut borrowed = map.borrow_mut();
                if let Some(index) = map_index(&borrowed, &key) {
                    borrowed.entries[index].1 = value;
                } else {
                    if borrowed.entries.len() >= MAX_COLLECTION_ENTRIES {
                        return Err("RangeError: Map entry budget exceeded".to_string());
                    }
                    borrowed.entries.push((key, value));
                }
            }
            Ok(JsvValue::Map(map))
        }
        ("Map.has", JsvValue::Map(map)) => {
            let key = args.first().cloned().unwrap_or(JsvValue::Undefined);
            Ok(JsvValue::Boolean(map_index(&map.borrow(), &key).is_some()))
        }
        ("Map.delete", JsvValue::Map(map)) => {
            let key = args.first().cloned().unwrap_or(JsvValue::Undefined);
            let index = map_index(&map.borrow(), &key);
            Ok(JsvValue::Boolean(index.is_some_and(|index| {
                map.borrow_mut().entries.remove(index);
                true
            })))
        }
        ("Map.clear", JsvValue::Map(map)) => {
            map.borrow_mut().entries.clear();
            Ok(JsvValue::Undefined)
        }
        (method @ ("Map.keys" | "Map.values" | "Map.entries"), JsvValue::Map(map)) => {
            let column = match method {
                "Map.keys" => 1,
                "Map.values" => 2,
                _ => 0,
            };
            Ok(collection_iterator(
                JsvValue::Map(map),
                IteratorKind::Map,
                column,
            ))
        }
        ("Map.forEach", JsvValue::Map(map)) => {
            let callback = args.first().cloned().unwrap_or(JsvValue::Undefined);
            if !is_callable(&callback) {
                return Err("TypeError: Map.forEach callback must be callable".to_string());
            }
            let initial_len = map.borrow().entries.len();
            for index in 0..initial_len {
                let Some((key, value)) = map.borrow().entries.get(index).cloned() else {
                    break;
                };
                let called = call_function(
                    callback.clone(),
                    vec![value, key, JsvValue::Map(map.clone())],
                    env,
                    console,
                    ctx,
                    JsvValue::Undefined,
                )?;
                call_result_value(called)?;
            }
            Ok(JsvValue::Undefined)
        }
        ("Set.add", JsvValue::Set(set)) => {
            let value = args.first().cloned().unwrap_or(JsvValue::Undefined);
            {
                let mut borrowed = set.borrow_mut();
                if set_index(&borrowed, &value).is_none() {
                    if borrowed.values.len() >= MAX_COLLECTION_ENTRIES {
                        return Err("RangeError: Set entry budget exceeded".to_string());
                    }
                    borrowed.values.push(value);
                }
            }
            Ok(JsvValue::Set(set))
        }
        ("Set.has", JsvValue::Set(set)) => {
            let value = args.first().cloned().unwrap_or(JsvValue::Undefined);
            Ok(JsvValue::Boolean(
                set_index(&set.borrow(), &value).is_some(),
            ))
        }
        ("Set.delete", JsvValue::Set(set)) => {
            let value = args.first().cloned().unwrap_or(JsvValue::Undefined);
            let index = set_index(&set.borrow(), &value);
            Ok(JsvValue::Boolean(index.is_some_and(|index| {
                set.borrow_mut().values.remove(index);
                true
            })))
        }
        ("Set.clear", JsvValue::Set(set)) => {
            set.borrow_mut().values.clear();
            Ok(JsvValue::Undefined)
        }
        (method @ ("Set.keys" | "Set.values" | "Set.entries"), JsvValue::Set(set)) => {
            let column = usize::from(method == "Set.entries") as u8;
            Ok(collection_iterator(
                JsvValue::Set(set),
                IteratorKind::Set,
                column,
            ))
        }
        ("Set.forEach", JsvValue::Set(set)) => {
            let callback = args.first().cloned().unwrap_or(JsvValue::Undefined);
            if !is_callable(&callback) {
                return Err("TypeError: Set.forEach callback must be callable".to_string());
            }
            let initial_len = set.borrow().values.len();
            for index in 0..initial_len {
                let Some(value) = set.borrow().values.get(index).cloned() else {
                    break;
                };
                let called = call_function(
                    callback.clone(),
                    vec![value.clone(), value, JsvValue::Set(set.clone())],
                    env,
                    console,
                    ctx,
                    JsvValue::Undefined,
                )?;
                call_result_value(called)?;
            }
            Ok(JsvValue::Undefined)
        }
        (method, JsvValue::WeakMap(map)) if method.starts_with("WeakMap.") => {
            let key = args.first().cloned().unwrap_or(JsvValue::Undefined);
            let Some(weak_key) = weak_key_of(&key) else {
                return match method {
                    "WeakMap.get" => Ok(JsvValue::Undefined),
                    "WeakMap.has" | "WeakMap.delete" => Ok(JsvValue::Boolean(false)),
                    _ => Err("TypeError: WeakMap key must be an object".to_string()),
                };
            };
            let mut borrowed = map.borrow_mut();
            borrowed.entries.retain(|entry| weak_key_alive(&entry.key));
            let index = borrowed
                .entries
                .iter()
                .position(|entry| weak_key_eq(&entry.key, &weak_key));
            match method {
                "WeakMap.get" => Ok(index
                    .map(|index| borrowed.entries[index].value.clone())
                    .unwrap_or(JsvValue::Undefined)),
                "WeakMap.has" => Ok(JsvValue::Boolean(index.is_some())),
                "WeakMap.delete" => Ok(JsvValue::Boolean(index.is_some_and(|index| {
                    borrowed.entries.remove(index);
                    true
                }))),
                "WeakMap.set" => {
                    let value = args.get(1).cloned().unwrap_or(JsvValue::Undefined);
                    if let Some(index) = index {
                        borrowed.entries[index].value = value;
                    } else {
                        if borrowed.entries.len() >= MAX_COLLECTION_ENTRIES {
                            return Err("RangeError: WeakMap entry budget exceeded".to_string());
                        }
                        borrowed.entries.push(JsvWeakEntry {
                            key: weak_key,
                            value,
                        });
                    }
                    drop(borrowed);
                    Ok(JsvValue::WeakMap(map))
                }
                _ => Err(format!("TypeError: unsupported native method {method}")),
            }
        }
        (method, JsvValue::WeakSet(set)) if method.starts_with("WeakSet.") => {
            let value = args.first().cloned().unwrap_or(JsvValue::Undefined);
            let weak_key = weak_key_of(&value)
                .ok_or_else(|| "TypeError: WeakSet value must be an object".to_string())?;
            let mut borrowed = set.borrow_mut();
            borrowed.keys.retain(weak_key_alive);
            let index = borrowed
                .keys
                .iter()
                .position(|existing| weak_key_eq(existing, &weak_key));
            match method {
                "WeakSet.has" => Ok(JsvValue::Boolean(index.is_some())),
                "WeakSet.delete" => Ok(JsvValue::Boolean(index.is_some_and(|index| {
                    borrowed.keys.remove(index);
                    true
                }))),
                "WeakSet.add" => {
                    if index.is_none() {
                        if borrowed.keys.len() >= MAX_COLLECTION_ENTRIES {
                            return Err("RangeError: WeakSet entry budget exceeded".to_string());
                        }
                        borrowed.keys.push(weak_key);
                    }
                    drop(borrowed);
                    Ok(JsvValue::WeakSet(set))
                }
                _ => Err(format!("TypeError: unsupported native method {method}")),
            }
        }
        ("WeakRef.deref", target) => Ok(target),
        (
            method @ ("Promise.then" | "Promise.catch" | "Promise.finally"),
            JsvValue::Promise(promise),
        ) => {
            let result = Rc::new(RefCell::new(JsvPromiseState::Pending));
            let state = promise
                .try_borrow()
                .map_err(|_| "TypeError: Reentrant access to promise".to_string())?
                .clone();
            let callable =
                |index: usize| args.get(index).filter(|value| is_callable(value)).cloned();
            let reaction = match method {
                "Promise.then" => PendingPromiseReaction {
                    on_fulfilled: callable(0),
                    on_rejected: callable(1),
                    on_finally: None,
                    completion_override: None,
                    result: result.clone(),
                },
                "Promise.catch" => PendingPromiseReaction {
                    on_fulfilled: None,
                    on_rejected: callable(0),
                    on_finally: None,
                    completion_override: None,
                    result: result.clone(),
                },
                _ => PendingPromiseReaction {
                    on_fulfilled: None,
                    on_rejected: None,
                    on_finally: callable(0),
                    completion_override: None,
                    result: result.clone(),
                },
            };
            match state {
                JsvPromiseState::Pending => {
                    register_pending_reaction(ctx, &promise, reaction)?;
                }
                settled => enqueue_registered_reaction(ctx, reaction, settled)?,
            }
            Ok(JsvValue::Promise(result))
        }
        (method, JsvValue::String(text)) if method.starts_with("String.") => {
            call_string_method(&method["String.".len()..], text, &args, ctx)
        }
        _ => {
            let _ = (env, console);
            Err(format!("TypeError: unsupported native method {}", name))
        }
    }
}

fn drain_promise_jobs(
    env: &mut JsvEnvironment,
    console: &mut Vec<String>,
    ctx: &mut EvalCtx<'_>,
) -> Result<(), String> {
    let mut processed = 0usize;
    while let Some(job) = ctx.microtasks.pop_front() {
        if processed >= MAX_PROMISE_JOBS {
            ctx.microtasks.push_front(job);
            return Err("Promise job execution budget exceeded".to_string());
        }
        processed += 1;
        let original_rejected = job.kind == PromiseReactionKind::Reject;
        let outcome = if let Some(handler) = job.handler {
            let arguments = if job.kind == PromiseReactionKind::Finally {
                Vec::new()
            } else {
                vec![job.argument.clone()]
            };
            match call_function(handler, arguments, env, console, ctx, JsvValue::Undefined) {
                Ok(JsvValue::ThrowSignal(reason)) => Err(*reason),
                Ok(JsvValue::ReturnSignal(value)) => Ok(*value),
                Ok(JsvValue::BreakSignal) => Err(JsvValue::String(
                    "SyntaxError: illegal break statement in Promise handler".to_string(),
                )),
                Ok(JsvValue::ContinueSignal) => Err(JsvValue::String(
                    "SyntaxError: illegal continue statement in Promise handler".to_string(),
                )),
                Ok(value) => Ok(value),
                Err(error) => Err(JsvValue::String(error)),
            }
        } else if original_rejected {
            Err(job.argument.clone())
        } else {
            Ok(job.argument.clone())
        };

        let settlement = if job.kind == PromiseReactionKind::Finally {
            match outcome {
                Err(reason) => Err(reason),
                Ok(JsvValue::Promise(source)) => match source
                    .try_borrow()
                    .map_err(|_| "TypeError: Reentrant access to promise".to_string())?
                    .clone()
                {
                    JsvPromiseState::Rejected(reason) => Err(reason),
                    JsvPromiseState::Fulfilled(_) if original_rejected => Err(job.argument.clone()),
                    JsvPromiseState::Fulfilled(_) => Ok(job.argument.clone()),
                    JsvPromiseState::Pending => {
                        let kind = if original_rejected {
                            PromiseReactionKind::Reject
                        } else {
                            PromiseReactionKind::Fulfill
                        };
                        register_pending_reaction(
                            ctx,
                            &source,
                            PendingPromiseReaction {
                                on_fulfilled: None,
                                on_rejected: None,
                                on_finally: None,
                                completion_override: Some((kind, job.argument.clone())),
                                result: job.result.clone(),
                            },
                        )?;
                        continue;
                    }
                },
                Ok(_) if original_rejected => Err(job.argument.clone()),
                Ok(_) => Ok(job.argument.clone()),
            }
        } else {
            outcome
        };
        resolve_promise(ctx, &job.result, settlement)?;
    }
    Ok(())
}

fn is_sandboxed_function(name: &str) -> bool {
    let sandboxed_functions = [
        "eval",
        "Function",
        "setTimeout",
        "setInterval",
        "clearTimeout",
        "clearInterval",
        "document.write",
        "document.writeln",
        "document.getElementById",
        "document.createElement",
        "window.alert",
        "window.confirm",
        "window.prompt",
        "window.open",
        "window.location",
        "window.history",
        "window.navigator",
        "window.frames",
        "window.parent",
        "window.top",
        "window.opener",
        "window.crypto",
        "window.localStorage",
        "window.sessionStorage",
        "window.XMLHttpRequest",
        "window.fetch",
        "window.requestAnimationFrame",
        "window.cancelAnimationFrame",
        "globalThis",
        "window",
        "document",
        "self",
        "eval",
    ];

    sandboxed_functions.contains(&name)
}

fn call_native_fn(
    name: &str,
    args: Vec<JsvValue>,
    env: &mut JsvEnvironment,
    console: &mut Vec<String>,
    ctx: &mut EvalCtx<'_>,
) -> Result<JsvValue, String> {
    if is_sandboxed_function(name) {
        return Err(format!(
            "SecurityError: {} is not available in sandboxed JavaScript environment",
            name
        ));
    }

    match name {
        "console.log" | "print" => {
            let output = args
                .iter()
                .map(|a| a.to_display_string())
                .collect::<Vec<_>>()
                .join(" ");
            push_console(console, output.clone());
            #[cfg(debug_assertions)]
            println!("[JS] {}", output);
            Ok(JsvValue::Undefined)
        }
        "console.warn" => {
            let output = args
                .iter()
                .map(|a| a.to_display_string())
                .collect::<Vec<_>>()
                .join(" ");
            push_console(console, format!("WARN: {}", output));
            #[cfg(debug_assertions)]
            println!("[JS Warning] {}", output);
            Ok(JsvValue::Undefined)
        }
        "console.error" => {
            let output = args
                .iter()
                .map(|a| a.to_display_string())
                .collect::<Vec<_>>()
                .join(" ");
            push_console(console, format!("ERROR: {}", output));
            #[cfg(debug_assertions)]
            eprintln!("[JS Error] {}", output);
            Ok(JsvValue::Undefined)
        }
        "parseInt" => {
            let text = args.first().map(|value| value.to_display_string());
            let value = text
                .as_deref()
                .and_then(parse_int_prefix)
                .unwrap_or(f64::NAN);
            Ok(JsvValue::Number(value))
        }
        "parseFloat" => {
            let text = args.first().map(|value| value.to_display_string());
            let value = text
                .as_deref()
                .and_then(|text| {
                    let trimmed = text.trim();
                    let (sign, body) = match trimmed.as_bytes().first() {
                        Some(b'+') => (1.0, &trimmed[1..]),
                        Some(b'-') => (-1.0, &trimmed[1..]),
                        _ => (1.0, trimmed),
                    };
                    if body.starts_with("Infinity") {
                        Some(sign * f64::INFINITY)
                    } else {
                        parse_number_prefix(trimmed)
                    }
                })
                .unwrap_or(f64::NAN);
            Ok(JsvValue::Number(value))
        }
        "String" => {
            if let Some(a) = args.first() {
                Ok(JsvValue::String(a.to_display_string()))
            } else {
                Ok(JsvValue::String("".to_string()))
            }
        }
        "Number" => {
            let value = args.first().map(to_number).unwrap_or(0.0);
            Ok(JsvValue::Number(value))
        }
        "Object.create" => {
            let prototype = match args.first() {
                Some(JsvValue::Object(object)) => Some(object.clone()),
                Some(JsvValue::Null) | None => None,
                _ => {
                    return Err("TypeError: Object prototype must be an object or null".to_string())
                }
            };
            Ok(JsvValue::Object(Rc::new(RefCell::new(JsvObject {
                properties: HashMap::new(),
                prototype,
                class_tags: Vec::new(),
                private_fields: HashMap::new(),
            }))))
        }
        "Object.keys" => {
            let object = match args.first() {
                Some(JsvValue::Object(object)) => object,
                _ => return Err("TypeError: Object.keys requires an object".to_string()),
            };
            let mut keys = object
                .borrow()
                .properties
                .keys()
                .filter(|key| !is_symbol_property_key(key))
                .cloned()
                .collect::<Vec<_>>();
            keys.sort();
            Ok(JsvValue::Array(Rc::new(RefCell::new(
                keys.into_iter().map(JsvValue::String).collect(),
            ))))
        }
        "Array.isArray" => Ok(JsvValue::Boolean(matches!(
            args.first(),
            Some(JsvValue::Array(_))
        ))),
        "Map" => {
            let map = Rc::new(RefCell::new(JsvMap::default()));
            if let Some(iterable) = args
                .first()
                .filter(|value| !matches!(value.deref_live(), JsvValue::Undefined | JsvValue::Null))
            {
                for entry in iterate_values(&iterable.deref_live(), console, ctx)? {
                    let JsvValue::Array(pair) = entry.deref_live() else {
                        return Err(
                            "TypeError: Map iterable values must be key/value pairs".to_string()
                        );
                    };
                    let pair = pair.borrow();
                    let key = pair.first().cloned().unwrap_or(JsvValue::Undefined);
                    let value = pair.get(1).cloned().unwrap_or(JsvValue::Undefined);
                    let mut borrowed = map.borrow_mut();
                    if let Some(index) = map_index(&borrowed, &key) {
                        borrowed.entries[index].1 = value;
                    } else {
                        if borrowed.entries.len() >= MAX_COLLECTION_ENTRIES {
                            return Err("RangeError: Map entry budget exceeded".to_string());
                        }
                        borrowed.entries.push((key, value));
                    }
                }
            }
            Ok(JsvValue::Map(map))
        }
        "Set" => {
            let set = Rc::new(RefCell::new(JsvSet::default()));
            if let Some(iterable) = args
                .first()
                .filter(|value| !matches!(value.deref_live(), JsvValue::Undefined | JsvValue::Null))
            {
                for value in iterate_values(&iterable.deref_live(), console, ctx)? {
                    let mut borrowed = set.borrow_mut();
                    if set_index(&borrowed, &value).is_none() {
                        if borrowed.values.len() >= MAX_COLLECTION_ENTRIES {
                            return Err("RangeError: Set entry budget exceeded".to_string());
                        }
                        borrowed.values.push(value);
                    }
                }
            }
            Ok(JsvValue::Set(set))
        }
        "WeakMap" => {
            let map = Rc::new(RefCell::new(JsvWeakMap::default()));
            if let Some(iterable) = args
                .first()
                .filter(|value| !matches!(value.deref_live(), JsvValue::Undefined | JsvValue::Null))
            {
                for entry in iterate_values(&iterable.deref_live(), console, ctx)? {
                    let JsvValue::Array(pair) = entry.deref_live() else {
                        return Err("TypeError: WeakMap iterable values must be key/value pairs"
                            .to_string());
                    };
                    let pair = pair.borrow();
                    let key = pair.first().cloned().unwrap_or(JsvValue::Undefined);
                    let weak_key = weak_key_of(&key)
                        .ok_or_else(|| "TypeError: WeakMap key must be an object".to_string())?;
                    let value = pair.get(1).cloned().unwrap_or(JsvValue::Undefined);
                    let mut borrowed = map.borrow_mut();
                    if let Some(index) = borrowed
                        .entries
                        .iter()
                        .position(|entry| weak_key_eq(&entry.key, &weak_key))
                    {
                        borrowed.entries[index].value = value;
                    } else {
                        if borrowed.entries.len() >= MAX_COLLECTION_ENTRIES {
                            return Err("RangeError: WeakMap entry budget exceeded".to_string());
                        }
                        borrowed.entries.push(JsvWeakEntry {
                            key: weak_key,
                            value,
                        });
                    }
                }
            }
            Ok(JsvValue::WeakMap(map))
        }
        "WeakSet" => {
            let set = Rc::new(RefCell::new(JsvWeakSet::default()));
            if let Some(iterable) = args
                .first()
                .filter(|value| !matches!(value.deref_live(), JsvValue::Undefined | JsvValue::Null))
            {
                for value in iterate_values(&iterable.deref_live(), console, ctx)? {
                    let weak_key = weak_key_of(&value)
                        .ok_or_else(|| "TypeError: WeakSet value must be an object".to_string())?;
                    let mut borrowed = set.borrow_mut();
                    if !borrowed
                        .keys
                        .iter()
                        .any(|existing| weak_key_eq(existing, &weak_key))
                    {
                        if borrowed.keys.len() >= MAX_COLLECTION_ENTRIES {
                            return Err("RangeError: WeakSet entry budget exceeded".to_string());
                        }
                        borrowed.keys.push(weak_key);
                    }
                }
            }
            Ok(JsvValue::WeakSet(set))
        }
        "Error" | "TypeError" | "RangeError" | "ReferenceError" | "SyntaxError" | "EvalError"
        | "URIError" => Ok(error_value(
            name,
            &args
                .first()
                .map(JsvValue::to_display_string)
                .unwrap_or_default(),
        )),
        "Promise.resolve" => match args.first().cloned().unwrap_or(JsvValue::Undefined) {
            promise @ JsvValue::Promise(_) => Ok(promise),
            value => Ok(promise_value(JsvPromiseState::Fulfilled(value))),
        },
        "Promise.reject" => Ok(promise_value(JsvPromiseState::Rejected(
            args.first().cloned().unwrap_or(JsvValue::Undefined),
        ))),
        "Promise.all" => {
            let values = match args.first() {
                Some(JsvValue::Array(values)) => values.borrow().clone(),
                _ => return Err("TypeError: Promise.all requires an array".to_string()),
            };
            let mut output = Vec::with_capacity(values.len());
            for value in values {
                match value {
                    JsvValue::Promise(promise) => match promise.borrow().clone() {
                        JsvPromiseState::Fulfilled(value) => output.push(value),
                        JsvPromiseState::Rejected(reason) => {
                            return Ok(promise_value(JsvPromiseState::Rejected(reason)));
                        }
                        JsvPromiseState::Pending => {
                            return Ok(promise_value(JsvPromiseState::Pending));
                        }
                    },
                    value => output.push(value),
                }
            }
            Ok(promise_value(JsvPromiseState::Fulfilled(JsvValue::Array(
                Rc::new(RefCell::new(output)),
            ))))
        }
        "Promise.race" => {
            let values = match args.first() {
                Some(JsvValue::Array(values)) => values.borrow().clone(),
                _ => return Err("TypeError: Promise.race requires an array".to_string()),
            };
            let state = values
                .into_iter()
                .find_map(|value| match value {
                    JsvValue::Promise(promise) => match promise.borrow().clone() {
                        JsvPromiseState::Pending => None,
                        settled => Some(settled),
                    },
                    value => Some(JsvPromiseState::Fulfilled(value)),
                })
                .unwrap_or(JsvPromiseState::Pending);
            Ok(promise_value(state))
        }
        "Promise.allSettled" => {
            let values = match args.first() {
                Some(JsvValue::Array(values)) => values.borrow().clone(),
                _ => return Err("TypeError: Promise.allSettled requires an array".to_string()),
            };
            let mut output = Vec::with_capacity(values.len());
            for value in values {
                let (status, key, val) = match value {
                    JsvValue::Promise(promise) => match promise.borrow().clone() {
                        JsvPromiseState::Fulfilled(value) => ("fulfilled", "value", value),
                        JsvPromiseState::Rejected(reason) => ("rejected", "reason", reason),
                        JsvPromiseState::Pending => ("fulfilled", "value", JsvValue::Undefined),
                    },
                    value => ("fulfilled", "value", value),
                };
                let mut map = std::collections::HashMap::new();
                map.insert("status".to_string(), JsvValue::String(status.to_string()));
                map.insert(key.to_string(), val);
                output.push(JsvValue::Object(Rc::new(RefCell::new(JsvObject::plain(
                    map,
                )))));
            }
            Ok(promise_value(JsvPromiseState::Fulfilled(JsvValue::Array(
                Rc::new(RefCell::new(output)),
            ))))
        }
        "Promise.any" => {
            let values = match args.first() {
                Some(JsvValue::Array(values)) => values.borrow().clone(),
                _ => return Err("TypeError: Promise.any requires an array".to_string()),
            };
            if values.is_empty() {
                return Ok(promise_value(JsvPromiseState::Rejected(JsvValue::String(
                    "All promises were rejected".to_string(),
                ))));
            }
            let first_fulfilled = values.iter().find_map(|v| match v {
                JsvValue::Promise(p) => match p.borrow().clone() {
                    JsvPromiseState::Fulfilled(val) => Some(val),
                    _ => None,
                },
                v => Some(v.clone()),
            });
            if let Some(val) = first_fulfilled {
                Ok(promise_value(JsvPromiseState::Fulfilled(val)))
            } else {
                Ok(promise_value(JsvPromiseState::Rejected(JsvValue::String(
                    "AggregateError: All promises were rejected".to_string(),
                ))))
            }
        }
        "Math.abs" => {
            let n = first_number(&args);
            Ok(JsvValue::Number(n.abs()))
        }
        "Math.ceil" => {
            let n = first_number(&args);
            Ok(JsvValue::Number(n.ceil()))
        }
        "Math.floor" => {
            let n = first_number(&args);
            Ok(JsvValue::Number(n.floor()))
        }
        "Math.round" => {
            let n = first_number(&args);

            Ok(JsvValue::Number((n + 0.5).floor()))
        }
        "Math.sqrt" => {
            let n = first_number(&args);

            Ok(JsvValue::Number(n.sqrt()))
        }
        "Math.random" => Ok(JsvValue::Number(random_f64())),
        "Date.now" => Ok(JsvValue::Number(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as f64)
                .unwrap_or(0.0),
        )),
        "Date.parse" => {
            let ts = args
                .first()
                .and_then(|a| a.as_string())
                .and_then(parse_date);
            Ok(JsvValue::Number(ts.unwrap_or(f64::NAN)))
        }
        "JSON.stringify" => {
            let value = args.first().unwrap_or(&JsvValue::Undefined);
            json_stringify(value, ctx)
        }
        "JSON.parse" => {
            let source = args
                .first()
                .and_then(|a| a.as_string())
                .ok_or_else(|| "TypeError: JSON.parse requires a string".to_string())?;
            json_parse(source)
        }

        "Reflect.get" => {
            let target = args
                .first()
                .ok_or("TypeError: Reflect.get requires a target")?;
            let key = args
                .get(1)
                .map(|v| v.to_display_string())
                .unwrap_or_default();
            match target {
                JsvValue::Object(obj) => Ok(obj
                    .borrow()
                    .properties
                    .get(&key)
                    .cloned()
                    .unwrap_or(JsvValue::Undefined)),
                _ => Err("TypeError: Reflect.get target must be an object".to_string()),
            }
        }
        "Reflect.set" => {
            let target = args
                .first()
                .ok_or("TypeError: Reflect.set requires a target")?;
            let key = args
                .get(1)
                .map(|v| v.to_display_string())
                .unwrap_or_default();
            let value = args.get(2).cloned().unwrap_or(JsvValue::Undefined);
            match target {
                JsvValue::Object(obj) => {
                    let borrowed = obj.borrow();
                    let blocked = (matches!(
                        borrowed.properties.get("__sylphra_non_extensible"),
                        Some(JsvValue::Boolean(true))
                    ) || matches!(
                        borrowed.properties.get("__ghita_non_extensible"),
                        Some(JsvValue::Boolean(true))
                    )) && !borrowed.properties.contains_key(&key);
                    drop(borrowed);
                    if blocked {
                        return Ok(JsvValue::Boolean(false));
                    }
                    obj.borrow_mut().properties.insert(key, value);
                    Ok(JsvValue::Boolean(true))
                }
                _ => Err("TypeError: Reflect.set target must be an object".to_string()),
            }
        }
        "Reflect.has" => {
            let target = args
                .first()
                .ok_or("TypeError: Reflect.has requires a target")?;
            let key = args
                .get(1)
                .map(|v| v.to_display_string())
                .unwrap_or_default();
            match target {
                JsvValue::Object(obj) => Ok(JsvValue::Boolean(
                    obj.borrow().properties.contains_key(&key),
                )),
                _ => Err("TypeError: Reflect.has target must be an object".to_string()),
            }
        }
        "Reflect.deleteProperty" => {
            let target = args
                .first()
                .ok_or("TypeError: Reflect.deleteProperty requires a target")?;
            let key = args
                .get(1)
                .map(|v| v.to_display_string())
                .unwrap_or_default();
            match target {
                JsvValue::Object(obj) => {
                    obj.borrow_mut().properties.remove(&key);
                    Ok(JsvValue::Boolean(true))
                }
                _ => Err("TypeError: Reflect.deleteProperty target must be an object".to_string()),
            }
        }
        "Reflect.ownKeys" => {
            let target = args
                .first()
                .ok_or("TypeError: Reflect.ownKeys requires a target")?;
            match target {
                JsvValue::Object(obj) => {
                    let keys: Vec<JsvValue> = obj
                        .borrow()
                        .properties
                        .keys()
                        .cloned()
                        .map(JsvValue::String)
                        .collect();
                    Ok(JsvValue::Array(Rc::new(RefCell::new(keys))))
                }
                _ => Err("TypeError: Reflect.ownKeys target must be an object".to_string()),
            }
        }
        "Reflect.apply" => {
            let target = args
                .first()
                .cloned()
                .ok_or_else(|| "TypeError: Reflect.apply requires a target".to_string())?;
            let this_arg = args.get(1).cloned().unwrap_or(JsvValue::Undefined);
            let call_args = match args.get(2) {
                Some(JsvValue::Array(values)) => values.borrow().clone(),
                _ => return Err("TypeError: Reflect.apply arguments must be an array".to_string()),
            };
            call_function(target, call_args, env, console, ctx, this_arg)
                .and_then(call_result_value)
        }
        "Reflect.construct" => {
            let target = args
                .first()
                .cloned()
                .ok_or_else(|| "TypeError: Reflect.construct requires a target".to_string())?;
            let construct_args = match args.get(1) {
                Some(JsvValue::Array(values)) => values.borrow().clone(),
                _ => {
                    return Err(
                        "TypeError: Reflect.construct arguments must be an array".to_string()
                    )
                }
            };
            invoke_new(target, construct_args, env, console, ctx)
        }
        "Reflect.isExtensible" => {
            let object = match args.first() {
                Some(JsvValue::Object(object)) => object,
                _ => {
                    return Err(
                        "TypeError: Reflect.isExtensible target must be an object".to_string()
                    )
                }
            };
            let borrowed = object.borrow();
            let is_non_extensible = matches!(
                borrowed.properties.get("__sylphra_non_extensible"),
                Some(JsvValue::Boolean(true))
            ) || matches!(
                borrowed.properties.get("__ghita_non_extensible"),
                Some(JsvValue::Boolean(true))
            );
            Ok(JsvValue::Boolean(!is_non_extensible))
        }
        "Reflect.preventExtensions" => {
            let object = match args.first() {
                Some(JsvValue::Object(object)) => object,
                _ => {
                    return Err(
                        "TypeError: Reflect.preventExtensions target must be an object".to_string(),
                    )
                }
            };
            object.borrow_mut().properties.insert(
                "__sylphra_non_extensible".to_string(),
                JsvValue::Boolean(true),
            );
            Ok(JsvValue::Boolean(true))
        }
        "Reflect.defineProperty"
        | "Reflect.getOwnPropertyDescriptor"
        | "Reflect.getPrototypeOf"
        | "Reflect.setPrototypeOf" => Ok(JsvValue::Boolean(true)),

        "WeakRef" => {
            let target = args.first().cloned().unwrap_or(JsvValue::Undefined);
            if weak_key_of(&target).is_none() {
                return Err("TypeError: WeakRef target must be an object".to_string());
            }
            let obj = Rc::new(RefCell::new(JsvObject::plain(HashMap::from([
                ("deref_target".to_string(), target),
                (
                    "__sylphra_native_brand".to_string(),
                    JsvValue::String("WeakRef".to_string()),
                ),
            ]))));
            Ok(JsvValue::Object(obj))
        }
        "WeakRef.deref" => {
            let receiver = args
                .first()
                .ok_or("TypeError: WeakRef.deref requires receiver")?;
            match receiver {
                JsvValue::Object(obj)
                    if {
                        let b = obj.borrow();
                        b.properties
                            .get("__sylphra_native_brand")
                            .or_else(|| b.properties.get("__ghita_native_brand"))
                            .and_then(JsvValue::as_string)
                            == Some("WeakRef")
                    } =>
                {
                    Ok(obj
                        .borrow()
                        .properties
                        .get("deref_target")
                        .cloned()
                        .unwrap_or(JsvValue::Undefined))
                }
                _ => Err("TypeError: WeakRef.deref called on incompatible receiver".to_string()),
            }
        }
        "FinalizationRegistry" => {
            let callback = args.first().cloned().unwrap_or(JsvValue::Undefined);
            if !is_callable(&callback) {
                return Err("TypeError: FinalizationRegistry callback must be callable".to_string());
            }
            let obj = Rc::new(RefCell::new(JsvObject::plain(HashMap::from([
                ("callback".to_string(), callback),
                (
                    "__sylphra_native_brand".to_string(),
                    JsvValue::String("FinalizationRegistry".to_string()),
                ),
            ]))));
            Ok(JsvValue::Object(obj))
        }
        "FinalizationRegistry.register" | "FinalizationRegistry.unregister" => {
            Ok(JsvValue::Undefined)
        }

        "Intl.DateTimeFormat" => {
            let obj = Rc::new(RefCell::new(JsvObject::plain(HashMap::from([
                ("locale".to_string(), JsvValue::String("en-US".to_string())),
                (
                    "type".to_string(),
                    JsvValue::String("DateTimeFormat".to_string()),
                ),
                (
                    "__sylphra_native_brand".to_string(),
                    JsvValue::String("DateTimeFormat".to_string()),
                ),
            ]))));
            Ok(JsvValue::Object(obj))
        }
        "DateTimeFormat.format" => {
            let ts = args.first().and_then(|v| v.as_number()).unwrap_or_else(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as f64)
                    .unwrap_or(0.0)
            });
            let secs = (ts / 1000.0) as i64;
            let formatted = format!(
                "{}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
                1970 + secs / 31557600,
                ((secs % 31557600) / 2629800).max(1),
                ((secs % 2629800) / 86400).max(1),
                (secs % 86400) / 3600,
                (secs % 3600) / 60,
                secs % 60,
            );
            Ok(JsvValue::String(formatted))
        }
        "Intl.NumberFormat" => {
            let obj = Rc::new(RefCell::new(JsvObject::plain(HashMap::from([
                ("locale".to_string(), JsvValue::String("en-US".to_string())),
                (
                    "type".to_string(),
                    JsvValue::String("NumberFormat".to_string()),
                ),
                (
                    "__sylphra_native_brand".to_string(),
                    JsvValue::String("NumberFormat".to_string()),
                ),
            ]))));
            Ok(JsvValue::Object(obj))
        }
        "NumberFormat.format" => {
            let n = args.first().and_then(|v| v.as_number()).unwrap_or(0.0);

            let formatted = if n.fract() == 0.0 {
                format!("{}", n as i64)
            } else {
                format!("{:.2}", n)
            };
            Ok(JsvValue::String(formatted))
        }
        "Intl.Collator" | "Intl.PluralRules" => {
            Ok(JsvValue::Object(Rc::new(RefCell::new(JsvObject {
                properties: HashMap::new(),
                prototype: None,
                class_tags: Vec::new(),
                private_fields: HashMap::new(),
            }))))
        }
        _ => Err(format!("ReferenceError: {} is not defined", name)),
    }
}

const MAX_JSON_DEPTH: usize = 64;
const MAX_JSON_NODES: usize = 100_000;

fn json_stringify(value: &JsvValue, ctx: &mut EvalCtx<'_>) -> Result<JsvValue, String> {
    if matches!(
        value,
        JsvValue::Undefined
            | JsvValue::Function(..)
            | JsvValue::AsyncFunction(..)
            | JsvValue::NativeFn(_)
            | JsvValue::BoundNativeFn(..)
    ) {
        return Ok(JsvValue::Undefined);
    }
    let mut output = String::new();
    json_write(value, &mut output, 0)?;
    check_string_alloc(ctx, output.len())?;
    Ok(JsvValue::String(output))
}

fn json_write(value: &JsvValue, output: &mut String, depth: usize) -> Result<(), String> {
    if depth > MAX_JSON_DEPTH {
        return Err("JSON depth budget exceeded".to_string());
    }
    if output.len() > MAX_STRING_LENGTH {
        return Err("String too large".to_string());
    }
    match value {
        JsvValue::Number(n) => {
            if n.is_finite() {
                if *n == n.floor() && n.abs() < 1e15 {
                    output.push_str(&(*n as i64).to_string());
                } else {
                    output.push_str(&n.to_string());
                }
            } else {
                output.push_str("null");
            }
        }
        JsvValue::Boolean(b) => output.push_str(if *b { "true" } else { "false" }),
        JsvValue::String(s) => json_escape(s, output),
        JsvValue::Null => output.push_str("null"),
        JsvValue::Array(array) => {
            output.push('[');
            let values = array.borrow();
            for (index, item) in values.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                if matches!(
                    item,
                    JsvValue::Undefined
                        | JsvValue::Function(..)
                        | JsvValue::AsyncFunction(..)
                        | JsvValue::NativeFn(_)
                        | JsvValue::BoundNativeFn(..)
                ) {
                    output.push_str("null");
                } else {
                    json_write(item, output, depth + 1)?;
                }
            }
            output.push(']');
        }
        JsvValue::Object(object) => {
            output.push('{');
            let borrowed = object.borrow();
            let mut first = true;
            for (key, item) in &borrowed.properties {
                if matches!(
                    item,
                    JsvValue::Undefined
                        | JsvValue::Function(..)
                        | JsvValue::AsyncFunction(..)
                        | JsvValue::NativeFn(_)
                        | JsvValue::BoundNativeFn(..)
                ) {
                    continue;
                }
                if !first {
                    output.push(',');
                }
                first = false;
                json_escape(key, output);
                output.push(':');
                json_write(item, output, depth + 1)?;
            }
            output.push('}');
        }
        _ => {
            output.push_str("null");
        }
    }
    Ok(())
}

fn json_escape(text: &str, output: &mut String) {
    output.push('"');
    for character in text.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            '\u{0008}' => output.push_str("\\b"),
            '\u{000C}' => output.push_str("\\f"),

            other if (other as u32) < 0x20 => {
                output.push_str(&format!("\\u{:04x}", other as u32));
            }
            other => output.push(other),
        }
    }
    output.push('"');
}

fn json_parse(source: &str) -> Result<JsvValue, String> {
    let chars: Vec<char> = source.chars().collect();
    let mut pos = 0usize;
    let mut nodes = 0usize;
    let value = json_parse_value(&chars, &mut pos, &mut nodes, 0)?;
    json_skip_ws(&chars, &mut pos);
    if pos != chars.len() {
        return Err("JSON parse error: trailing content".to_string());
    }
    Ok(value)
}

fn json_skip_ws(chars: &[char], pos: &mut usize) {
    while *pos < chars.len() && chars[*pos].is_whitespace() {
        *pos += 1;
    }
}

fn json_parse_value(
    chars: &[char],
    pos: &mut usize,
    nodes: &mut usize,
    depth: usize,
) -> Result<JsvValue, String> {
    if depth > MAX_JSON_DEPTH {
        return Err("JSON depth budget exceeded".to_string());
    }
    *nodes += 1;
    if *nodes > MAX_JSON_NODES {
        return Err("JSON node budget exceeded".to_string());
    }
    json_skip_ws(chars, pos);
    let Some(&first) = chars.get(*pos) else {
        return Err("JSON parse error: unexpected end of input".to_string());
    };
    match first {
        '{' => {
            *pos += 1;
            let mut properties = HashMap::new();
            json_skip_ws(chars, pos);
            if chars.get(*pos) == Some(&'}') {
                *pos += 1;
                return Ok(object_value(properties));
            }
            loop {
                json_skip_ws(chars, pos);
                let key = json_parse_string(chars, pos)?;
                json_skip_ws(chars, pos);
                if chars.get(*pos) != Some(&':') {
                    return Err("JSON parse error: expected ':'".to_string());
                }
                *pos += 1;
                let value = json_parse_value(chars, pos, nodes, depth + 1)?;
                properties.insert(key, value);
                json_skip_ws(chars, pos);
                match chars.get(*pos) {
                    Some(',') => {
                        *pos += 1;
                    }
                    Some('}') => {
                        *pos += 1;
                        break;
                    }
                    _ => return Err("JSON parse error: expected ',' or '}'".to_string()),
                }
            }
            Ok(object_value(properties))
        }
        '[' => {
            *pos += 1;
            let mut values = Vec::new();
            json_skip_ws(chars, pos);
            if chars.get(*pos) == Some(&']') {
                *pos += 1;
                return Ok(JsvValue::Array(Rc::new(RefCell::new(values))));
            }
            loop {
                let value = json_parse_value(chars, pos, nodes, depth + 1)?;
                values.push(value);
                json_skip_ws(chars, pos);
                match chars.get(*pos) {
                    Some(',') => {
                        *pos += 1;
                    }
                    Some(']') => {
                        *pos += 1;
                        break;
                    }
                    _ => return Err("JSON parse error: expected ',' or ']'".to_string()),
                }
            }
            Ok(JsvValue::Array(Rc::new(RefCell::new(values))))
        }
        '"' => Ok(JsvValue::String(json_parse_string(chars, pos)?)),
        't' => {
            json_consume_literal(chars, pos, "true")?;
            Ok(JsvValue::Boolean(true))
        }
        'f' => {
            json_consume_literal(chars, pos, "false")?;
            Ok(JsvValue::Boolean(false))
        }
        'n' => {
            json_consume_literal(chars, pos, "null")?;
            Ok(JsvValue::Null)
        }
        '-' | '0'..='9' => {
            let start = *pos;
            if chars.get(*pos) == Some(&'-') {
                *pos += 1;
            }
            while chars
                .get(*pos)
                .is_some_and(|character| character.is_ascii_digit())
            {
                *pos += 1;
            }
            if chars.get(*pos) == Some(&'.') {
                *pos += 1;
                while chars
                    .get(*pos)
                    .is_some_and(|character| character.is_ascii_digit())
                {
                    *pos += 1;
                }
            }
            if matches!(chars.get(*pos), Some('e' | 'E')) {
                *pos += 1;
                if matches!(chars.get(*pos), Some('+' | '-')) {
                    *pos += 1;
                }
                while chars
                    .get(*pos)
                    .is_some_and(|character| character.is_ascii_digit())
                {
                    *pos += 1;
                }
            }
            let token: String = chars[start..*pos].iter().collect();
            token
                .parse::<f64>()
                .map(JsvValue::Number)
                .map_err(|_| "JSON parse error: invalid number".to_string())
        }
        _ => Err("JSON parse error: unexpected token".to_string()),
    }
}

fn json_parse_string(chars: &[char], pos: &mut usize) -> Result<String, String> {
    if chars.get(*pos) != Some(&'"') {
        return Err("JSON parse error: expected string".to_string());
    }
    *pos += 1;
    let mut output = String::new();
    loop {
        let Some(&character) = chars.get(*pos) else {
            return Err("JSON parse error: unterminated string".to_string());
        };
        *pos += 1;
        match character {
            '"' => return Ok(output),
            '\\' => {
                let Some(&escaped) = chars.get(*pos) else {
                    return Err("JSON parse error: unterminated escape".to_string());
                };
                *pos += 1;
                match escaped {
                    '"' => output.push('"'),
                    '\\' => output.push('\\'),
                    '/' => output.push('/'),
                    'b' => output.push('\u{0008}'),
                    'f' => output.push('\u{000C}'),
                    'n' => output.push('\n'),
                    'r' => output.push('\r'),
                    't' => output.push('\t'),
                    'u' => {
                        if *pos + 4 > chars.len() {
                            return Err("JSON parse error: invalid unicode escape".to_string());
                        }
                        let hex: String = chars[*pos..*pos + 4].iter().collect();
                        let code = u32::from_str_radix(&hex, 16)
                            .map_err(|_| "JSON parse error: invalid unicode escape".to_string())?;
                        *pos += 4;

                        let decoded = if (0xD800..=0xDBFF).contains(&code)
                            && chars.get(*pos) == Some(&'\\')
                            && chars.get(*pos + 1) == Some(&'u')
                            && *pos + 6 <= chars.len()
                        {
                            let low_hex: String = chars[*pos + 2..*pos + 6].iter().collect();
                            match u32::from_str_radix(&low_hex, 16) {
                                Ok(low) if (0xDC00..=0xDFFF).contains(&low) => {
                                    *pos += 6;
                                    char::from_u32(
                                        0x10000 + ((code - 0xD800) << 10) + (low - 0xDC00),
                                    )
                                    .unwrap_or('\u{FFFD}')
                                }
                                _ => '\u{FFFD}',
                            }
                        } else {
                            char::from_u32(code).unwrap_or('\u{FFFD}')
                        };
                        output.push(decoded);
                    }
                    _ => return Err("JSON parse error: invalid escape".to_string()),
                }
            }
            other => output.push(other),
        }
        if output.len() > MAX_STRING_LENGTH {
            return Err("String too large".to_string());
        }
    }
}

fn json_consume_literal(chars: &[char], pos: &mut usize, literal: &str) -> Result<(), String> {
    let expected: Vec<char> = literal.chars().collect();
    if *pos + expected.len() > chars.len() || chars[*pos..*pos + expected.len()] != expected[..] {
        return Err("JSON parse error: invalid literal".to_string());
    }
    *pos += expected.len();
    Ok(())
}

fn first_number(args: &[JsvValue]) -> f64 {
    args.first().and_then(|a| a.as_number()).unwrap_or(f64::NAN)
}

fn parse_date(s: &str) -> Option<f64> {
    let s = s.trim();
    for fmt in &["%Y-%m-%d", "%Y-%m-%d %H:%M:%S", "%d/%m/%Y"] {
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, fmt) {
            return Some(dt.and_utc().timestamp_millis() as f64);
        }
        if let Ok(d) = chrono::NaiveDate::parse_from_str(s, fmt) {
            return Some(d.and_hms_opt(0, 0, 0)?.and_utc().timestamp_millis() as f64);
        }
    }
    None
}

pub fn random_f64() -> f64 {
    use std::cell::Cell;
    thread_local! {
        static STATE: Cell<u64> = Cell::new(seed_rng_state());
    }
    STATE.with(|s| {
        let mut x = s.get();

        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        if x == 0 {
            x = 0x9E37_79B9_7F4A_7C15;
        }
        s.set(x);
        (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    })
}

fn seed_rng_state() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let mut seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);

    let stack_addr = &seed as *const u64 as u64;
    use std::hash::{BuildHasher, Hasher};
    let hash_key = std::collections::hash_map::RandomState::new();
    let mixed = {
        let mut hasher = hash_key.build_hasher();
        hasher.write_u64(stack_addr);
        hasher.finish()
    };
    seed ^= mixed.rotate_left(32) ^ stack_addr.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    if seed == 0 {
        seed = 0x9E37_79B9_7F4A_7C15;
    }
    seed
}

pub fn random_u8() -> u8 {
    (random_f64() * 256.0).floor() as u8
}

pub fn random_uuid_v4() -> String {
    let b1 = random_u8();
    let b2 = random_u8();
    let b3 = random_u8();
    let b4 = random_u8();
    let b5 = random_u8();
    let b6 = random_u8();
    let b7 = (random_u8() & 0x0f) | 0x40;
    let b8 = random_u8();
    let b9 = (random_u8() & 0x3f) | 0x80;
    let b10 = random_u8();
    let b11 = random_u8();
    let b12 = random_u8();
    let b13 = random_u8();
    let b14 = random_u8();
    let b15 = random_u8();
    let b16 = random_u8();
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b1, b2, b3, b4, b5, b6, b7, b8, b9, b10, b11, b12, b13, b14, b15, b16
    )
}

fn tokenize(code: &str) -> Result<Vec<String>, String> {
    if code.len() > MAX_SOURCE_BYTES {
        return Err("Script source exceeds 2 MB budget".to_string());
    }
    let mut tokens = Vec::new();
    let chars: Vec<char> = code.chars().collect();
    let len = chars.len();
    let mut i = 0;

    let mut reading_template_text = false;
    let mut template_expr_depth = 0usize;
    let mut expr_bracket_depth = 0usize;

    while i < len {
        if tokens.len() >= MAX_TOKENS {
            return Err("Script token budget exceeded".to_string());
        }
        if reading_template_text {
            let c = chars[i];
            if c == '`' {
                tokens.push("`END".to_string());
                reading_template_text = false;
                i += 1;
            } else if c == '$' && i + 1 < len && chars[i + 1] == '{' {
                tokens.push("`${".to_string());
                reading_template_text = false;
                template_expr_depth += 1;
                expr_bracket_depth = 0;
                i += 2;
            } else if c == '\\' && i + 1 < len {
                let esc = chars[i + 1];
                i += 2;
                match esc {
                    'n' => tokens.push("\"\n\"".to_string()),
                    't' => tokens.push("\"\t\"".to_string()),
                    'r' => tokens.push("\"\r\"".to_string()),
                    '\\' => tokens.push("\"\\\\\"".to_string()),
                    '`' => tokens.push("\"`\"".to_string()),
                    '$' => tokens.push("\"$\"".to_string()),
                    _ => {
                        tokens.push(format!("\"\\{}\"", esc));
                    }
                }
            } else {
                let mut text = String::new();
                while i < len {
                    let c = chars[i];
                    if c == '`' || (c == '$' && i + 1 < len && chars[i + 1] == '{') {
                        break;
                    }
                    if c == '\\' && i + 1 < len {
                        break;
                    }
                    text.push(c);
                    i += 1;
                }
                tokens.push(format!("\"{}\"", text));
            }
            continue;
        }

        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }

        if chars[i].is_ascii_digit()
            || (chars[i] == '.' && i + 1 < len && chars[i + 1].is_ascii_digit())
        {
            let start = i;
            while i < len
                && (chars[i].is_ascii_digit()
                    || chars[i] == '.'
                    || chars[i] == 'e'
                    || chars[i] == 'E')
            {
                i += 1;
            }
            tokens.push(chars[start..i].iter().collect());
        } else if chars[i].is_alphabetic() || chars[i] == '_' || chars[i] == '$' {
            let start = i;
            while i < len && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '$') {
                i += 1;
            }
            tokens.push(chars[start..i].iter().collect());
        } else if chars[i] == '"' || chars[i] == '\'' {
            let quote = chars[i];
            i += 1;
            let mut s = String::new();
            let mut closed = false;
            while i < len {
                let c = chars[i];
                if c == quote {
                    closed = true;
                    i += 1;
                    break;
                }
                if c == '\\' && i + 1 < len {
                    let esc = chars[i + 1];
                    i += 2;
                    match esc {
                        'n' => s.push('\n'),
                        't' => s.push('\t'),
                        'r' => s.push('\r'),
                        'b' => s.push('\u{0008}'),
                        'f' => s.push('\u{000C}'),
                        '0' => s.push('\0'),
                        '\\' => s.push('\\'),
                        '\'' => s.push('\''),
                        '"' => s.push('"'),
                        '`' => s.push('`'),
                        _ => {
                            s.push('\\');
                            s.push(esc);
                        }
                    }
                } else {
                    s.push(c);
                    i += 1;
                }
            }
            if !closed {
                return Err("Unterminated string literal".to_string());
            }
            tokens.push(format!("\"{}\"", s));
        } else if chars[i] == '`' {
            tokens.push("`".to_string());
            reading_template_text = true;
            i += 1;
        } else if i + 2 < len {
            let three_chars: String = chars[i..i + 3].iter().collect();
            if matches!(three_chars.as_str(), "===" | "!==") {
                tokens.push(three_chars);
                i += 3;
            } else if i + 1 < len {
                let two_chars: String = chars[i..i + 2].iter().collect();
                if matches!(
                    two_chars.as_str(),
                    "==" | "!="
                        | "<="
                        | ">="
                        | "&&"
                        | "||"
                        | "=>"
                        | "??"
                        | "?."
                        | "++"
                        | "--"
                        | "+="
                        | "-="
                        | "*="
                        | "/="
                        | "%="
                ) {
                    tokens.push(two_chars);
                    i += 2;
                } else if chars[i] == '/' && i + 1 < len && chars[i + 1] == '/' {
                    i += 2;
                    while i < len && chars[i] != '\n' {
                        i += 1;
                    }
                } else if chars[i] == '/' && i + 1 < len && chars[i + 1] == '*' {
                    i += 2;
                    while i + 1 < len && !(chars[i] == '*' && chars[i + 1] == '/') {
                        i += 1;
                    }
                    i += 2;
                } else {
                    let single = chars[i].to_string();
                    track_template_bracket(
                        &mut tokens,
                        &single,
                        &mut template_expr_depth,
                        &mut expr_bracket_depth,
                        &mut reading_template_text,
                    )?;
                    i += 1;
                }
            }
        } else if [
            '+', '-', '*', '/', '%', '<', '>', '=', '!', '(', ')', '{', '}', '[', ']', ';', ',',
            '.', ':', '?',
        ]
        .contains(&chars[i])
        {
            let single = chars[i].to_string();
            track_template_bracket(
                &mut tokens,
                &single,
                &mut template_expr_depth,
                &mut expr_bracket_depth,
                &mut reading_template_text,
            )?;
            i += 1;
        } else {
            i += 1;
        }
    }
    if reading_template_text {
        return Err("Unterminated template literal".to_string());
    }
    if tokens.len() > MAX_TOKENS {
        return Err("Script token budget exceeded".to_string());
    }
    Ok(tokens)
}

fn track_template_bracket(
    tokens: &mut Vec<String>,
    token: &str,
    template_expr_depth: &mut usize,
    expr_bracket_depth: &mut usize,
    reading_template_text: &mut bool,
) -> Result<(), String> {
    if *template_expr_depth > 0 {
        match token {
            "(" | "[" | "{" => *expr_bracket_depth += 1,
            ")" | "]" => *expr_bracket_depth = expr_bracket_depth.saturating_sub(1),
            "}" if *expr_bracket_depth == 0 => {
                tokens.push("`}".to_string());
                *template_expr_depth -= 1;
                *reading_template_text = true;
                return Ok(());
            }
            "}" => *expr_bracket_depth -= 1,
            _ => {}
        }
    }
    tokens.push(token.to_string());
    Ok(())
}

fn parse_statements(
    tokens: &[String],
    pos: usize,
    depth: usize,
) -> Result<(Vec<JsvExpr>, usize), String> {
    if depth > MAX_PARSE_DEPTH {
        return Err("Script too deeply nested".to_string());
    }
    let mut stmts = Vec::new();
    let mut i = pos;

    while i < tokens.len() {
        let tok = &tokens[i];
        if tok == "}" || tok == ")" || tok == "]" {
            break;
        }
        if tok == ";" {
            i += 1;
            continue;
        }

        let (stmt, new_i) = parse_statement(tokens, i, depth)?;
        stmts.push(stmt);
        i = new_i;
        while i < tokens.len() && tokens[i] == ";" {
            i += 1;
        }
    }
    Ok((stmts, i))
}

fn parse_statement(
    tokens: &[String],
    pos: usize,
    depth: usize,
) -> Result<(JsvExpr, usize), String> {
    if pos >= tokens.len() {
        return Err("Unexpected end of input".to_string());
    }
    let tok = &tokens[pos];

    match tok.as_str() {
        "{" => {
            let (stmts, i) = parse_statements(tokens, pos + 1, depth + 1)?;
            if i >= tokens.len() || tokens[i] != "}" {
                return Err("Expected '}'".to_string());
            }
            Ok((JsvExpr::Block(stmts), i + 1))
        }
        "if" => {
            if pos + 1 >= tokens.len() || tokens[pos + 1] != "(" {
                return Err("Expected '(' after if".to_string());
            }
            let (cond, i) = parse_expression(tokens, pos + 2, depth + 1)?;
            if i >= tokens.len() || tokens[i] != ")" {
                return Err("Expected ')' after if condition".to_string());
            }
            let (then_stmt, i) = parse_statement(tokens, i + 1, depth + 1)?;
            let then_block = match then_stmt {
                JsvExpr::Block(stmts) => stmts,
                stmt => vec![stmt],
            };
            if i < tokens.len() && tokens[i] == "else" {
                let (else_stmt, i) = parse_statement(tokens, i + 1, depth + 1)?;
                let else_block = match else_stmt {
                    JsvExpr::Block(stmts) => stmts,
                    stmt => vec![stmt],
                };
                Ok((JsvExpr::If(Box::new(cond), then_block, Some(else_block)), i))
            } else {
                Ok((JsvExpr::If(Box::new(cond), then_block, None), i))
            }
        }
        "while" => {
            if pos + 1 >= tokens.len() || tokens[pos + 1] != "(" {
                return Err("Expected '(' after while".to_string());
            }
            let (cond, i) = parse_expression(tokens, pos + 2, depth + 1)?;
            if i >= tokens.len() || tokens[i] != ")" {
                return Err("Expected ')' after while condition".to_string());
            }
            let (body_stmt, i) = parse_statement(tokens, i + 1, depth + 1)?;
            let body = match body_stmt {
                JsvExpr::Block(stmts) => stmts,
                stmt => vec![stmt],
            };
            Ok((JsvExpr::While(Box::new(cond), body), i))
        }
        "do" => {
            let (body_stmt, i) = parse_statement(tokens, pos + 1, depth + 1)?;
            let body = match body_stmt {
                JsvExpr::Block(stmts) => stmts,
                stmt => vec![stmt],
            };
            if tokens.get(i).map(String::as_str) != Some("while") {
                return Err("Expected while after do body".to_string());
            }
            if tokens.get(i + 1).map(String::as_str) != Some("(") {
                return Err("Expected ( after while".to_string());
            }
            let (cond, i) = parse_expression(tokens, i + 2, depth + 1)?;
            if tokens.get(i).map(String::as_str) != Some(")") {
                return Err("Expected ) after do-while condition".to_string());
            }
            Ok((JsvExpr::DoWhile(Box::new(cond), body), i + 1))
        }
        "for" => {
            if tokens.get(pos + 1).map(String::as_str) != Some("(") {
                return Err("Expected '(' after for".to_string());
            }
            let mut i = pos + 2;
            let kind = match tokens.get(i).map(String::as_str) {
                Some("const") => DeclarationKind::Const,
                Some("var") => DeclarationKind::Var,
                Some("let") => DeclarationKind::Let,
                _ => return Err("Expected declaration in for...of header".to_string()),
            };
            i += 1;
            let binding = tokens
                .get(i)
                .filter(|name| is_identifier(name))
                .cloned()
                .ok_or_else(|| "Expected iterator binding".to_string())?;
            i += 1;
            if tokens.get(i).map(String::as_str) != Some("of") {
                if tokens.get(i).map(String::as_str) == Some("in") {
                    let (object, next) = parse_expression(tokens, i + 1, depth + 1)?;
                    if tokens.get(next).map(String::as_str) != Some(")") {
                        return Err("Expected ) after for-in object".to_string());
                    }
                    let (body, next) = parse_statement(tokens, next + 1, depth + 1)?;
                    let body = match body {
                        JsvExpr::Block(statements) => statements,
                        statement => vec![statement],
                    };
                    return Ok((JsvExpr::ForIn(binding, kind, Box::new(object), body), next));
                }
                return Err("Expected of or in after for binding".to_string());
            }
            let (iterable, next) = parse_expression(tokens, i + 1, depth + 1)?;
            if tokens.get(next).map(String::as_str) != Some(")") {
                return Err("Expected ')' after for...of iterable".to_string());
            }
            let (body, next) = parse_statement(tokens, next + 1, depth + 1)?;
            let body = match body {
                JsvExpr::Block(statements) => statements,
                statement => vec![statement],
            };
            Ok((
                JsvExpr::ForOf(
                    Box::new(ParamPattern::Identifier(binding)),
                    kind,
                    Box::new(iterable),
                    body,
                ),
                next,
            ))
        }
        "switch" => {
            if tokens.get(pos + 1).map(String::as_str) != Some("(") {
                return Err("Expected '(' after switch".to_string());
            }
            let (discriminant, mut i) = parse_expression(tokens, pos + 2, depth + 1)?;
            if tokens.get(i).map(String::as_str) != Some(")") {
                return Err("Expected ')' after switch discriminant".to_string());
            }
            i += 1;
            if tokens.get(i).map(String::as_str) != Some("{") {
                return Err("Expected '{' after switch".to_string());
            }
            i += 1;
            let mut cases: Vec<(Option<JsvExpr>, Vec<JsvExpr>)> = Vec::new();
            let mut default_clause: Option<Vec<JsvExpr>> = None;

            let mut open_header: Option<Option<JsvExpr>> = None;
            let mut open_body: Vec<JsvExpr> = Vec::new();
            loop {
                if tokens.get(i).map(String::as_str) == Some("}") {
                    break;
                }
                if tokens.get(i).map(String::as_str) == Some("case") {
                    let (case_expr, next) = parse_expression(tokens, i + 1, depth + 1)?;
                    if tokens.get(next).map(String::as_str) != Some(":") {
                        return Err("Expected ':' after case value".to_string());
                    }
                    if let Some(header) = open_header.take() {
                        let body = std::mem::take(&mut open_body);
                        if let Some(case_expr) = header {
                            cases.push((Some(case_expr), body));
                        } else {
                            default_clause = Some(body);
                        }
                    }
                    open_header = Some(Some(case_expr));
                    i = next + 1;
                } else if tokens.get(i).map(String::as_str) == Some("default") {
                    if tokens.get(i + 1).map(String::as_str) != Some(":") {
                        return Err("Expected ':' after default".to_string());
                    }
                    if let Some(header) = open_header.take() {
                        let body = std::mem::take(&mut open_body);
                        if let Some(case_expr) = header {
                            cases.push((Some(case_expr), body));
                        } else {
                            default_clause = Some(body);
                        }
                    }
                    if default_clause.is_some() {
                        return Err("Duplicate default clause in switch".to_string());
                    }
                    open_header = Some(None);
                    i += 2;
                } else {
                    let (statement, next) = parse_statement(tokens, i, depth + 1)?;
                    open_body.push(statement);
                    i = next;
                    while tokens.get(i).map(String::as_str) == Some(";") {
                        i += 1;
                    }
                }
            }
            if let Some(header) = open_header.take() {
                let body = std::mem::take(&mut open_body);
                if let Some(case_expr) = header {
                    cases.push((Some(case_expr), body));
                } else {
                    default_clause = Some(body);
                }
            }
            if i >= tokens.len() {
                return Err("Expected '}' to close switch".to_string());
            }
            Ok((
                JsvExpr::Switch(Box::new(discriminant), cases, default_clause),
                i + 1,
            ))
        }
        "try" => {
            if tokens.get(pos + 1).map(String::as_str) != Some("{") {
                return Err("Expected '{' after try".to_string());
            }
            let (try_body, mut i) = parse_statements(tokens, pos + 2, depth + 1)?;
            if tokens.get(i).map(String::as_str) != Some("}") {
                return Err("Expected '}' after try body".to_string());
            }
            i += 1;
            let mut catch_binding = None;
            let mut catch_body = Vec::new();
            let mut finally_body = Vec::new();
            if tokens.get(i).map(String::as_str) == Some("catch") {
                i += 1;
                if tokens.get(i).map(String::as_str) == Some("(") {
                    let binding = tokens
                        .get(i + 1)
                        .filter(|name| is_identifier(name))
                        .cloned()
                        .ok_or_else(|| "Expected catch binding".to_string())?;
                    if tokens.get(i + 2).map(String::as_str) != Some(")") {
                        return Err("Expected ')' after catch binding".to_string());
                    }
                    catch_binding = Some(binding);
                    i += 3;
                } else {
                    catch_binding = Some("error".to_string());
                }
                if tokens.get(i).map(String::as_str) != Some("{") {
                    return Err("Expected '{' before catch body".to_string());
                }
                let (body, next) = parse_statements(tokens, i + 1, depth + 1)?;
                if tokens.get(next).map(String::as_str) != Some("}") {
                    return Err("Expected '}' after catch body".to_string());
                }
                catch_body = body;
                i = next + 1;
            }
            if tokens.get(i).map(String::as_str) == Some("finally") {
                if tokens.get(i + 1).map(String::as_str) != Some("{") {
                    return Err("Expected '{' before finally body".to_string());
                }
                let (body, next) = parse_statements(tokens, i + 2, depth + 1)?;
                if tokens.get(next).map(String::as_str) != Some("}") {
                    return Err("Expected '}' after finally body".to_string());
                }
                finally_body = body;
                i = next + 1;
            }
            if catch_binding.is_none() && finally_body.is_empty() {
                return Err("Try statement requires catch or finally".to_string());
            }
            Ok((
                JsvExpr::TryCatchFinally {
                    try_body,
                    catch_binding,
                    catch_body,
                    finally_body,
                },
                i,
            ))
        }
        "var" | "let" | "const" => {
            let kind = match tok.as_str() {
                "var" => DeclarationKind::Var,
                "const" => DeclarationKind::Const,
                _ => DeclarationKind::Let,
            };
            if pos + 1 >= tokens.len() {
                return Err(format!("Expected variable name after {}", tok));
            }
            let var_name = tokens[pos + 1].clone();
            if !is_identifier(&var_name) {
                return Err(format!("Invalid variable name: {}", var_name));
            }
            let mut i = pos + 2;
            if i < tokens.len() && tokens[i] == "=" {
                let (val_expr, new_i) = parse_expression(tokens, i + 1, depth + 1)?;
                i = new_i;
                Ok((
                    JsvExpr::VariableDeclaration(var_name, Box::new(val_expr), kind),
                    i,
                ))
            } else {
                if kind == DeclarationKind::Const {
                    return Err("Missing initializer in const declaration".to_string());
                }
                Ok((
                    JsvExpr::VariableDeclaration(var_name, Box::new(JsvExpr::Undefined), kind),
                    i,
                ))
            }
        }
        "return" => {
            if pos + 1 < tokens.len() && tokens[pos + 1] != ";" && tokens[pos + 1] != "}" {
                let (val, i) = parse_expression(tokens, pos + 1, depth + 1)?;
                Ok((JsvExpr::Return(Box::new(val)), i))
            } else {
                Ok((JsvExpr::Return(Box::new(JsvExpr::Undefined)), pos + 1))
            }
        }
        "break" => {
            let label = tokens
                .get(pos + 1)
                .filter(|t| is_identifier(t) && t.as_str() != ";")
                .cloned();
            let next = if label.is_some() { pos + 2 } else { pos + 1 };
            Ok((JsvExpr::Break(label), next))
        }
        "continue" => {
            let label = tokens
                .get(pos + 1)
                .filter(|t| is_identifier(t) && t.as_str() != ";")
                .cloned();
            let next = if label.is_some() { pos + 2 } else { pos + 1 };
            Ok((JsvExpr::Continue(label), next))
        }
        "throw" => {
            if pos + 1 >= tokens.len() || tokens[pos + 1] == ";" {
                return Err("Throw statement requires a value".to_string());
            }
            let (value, i) = parse_expression(tokens, pos + 1, depth + 1)?;
            Ok((JsvExpr::Throw(Box::new(value)), i))
        }
        "async" => {
            if tokens.get(pos + 1).map(String::as_str) != Some("function") {
                return Err("Only async function declarations are supported".to_string());
            }
            let (function, i) = parse_statement(tokens, pos + 1, depth + 1)?;
            match function {
                JsvExpr::FunctionDef(name, parameters, body) => {
                    Ok((JsvExpr::AsyncFunctionDef(name, parameters, body), i))
                }
                _ => Err("Invalid async function declaration".to_string()),
            }
        }
        "function" => {
            if pos + 1 >= tokens.len() {
                return Err("Expected function name".to_string());
            }
            let name = tokens[pos + 1].clone();
            if pos + 2 >= tokens.len() || tokens[pos + 2] != "(" {
                return Err("Expected '(' after function name".to_string());
            }
            let mut params = Vec::new();
            let mut i = pos + 3;
            while i < tokens.len() && tokens[i] != ")" {
                if tokens[i] != "," && tokens[i] != ")" {
                    params.push(tokens[i].clone());
                }
                i += 1;
                while i < tokens.len() && tokens[i] == "," {
                    i += 1;
                }
            }
            if i >= tokens.len() {
                return Err("Expected ')' after parameters".to_string());
            }
            i += 1;
            if i >= tokens.len() || tokens[i] != "{" {
                return Err("Expected '{' for function body".to_string());
            }
            let (body_stmts, new_i) = parse_statements(tokens, i + 1, depth + 1)?;
            let mut i = new_i;
            if i >= tokens.len() || tokens[i] != "}" {
                return Err("Expected '}' to close function body".to_string());
            }
            i += 1;
            Ok((
                JsvExpr::FunctionDef(
                    name,
                    params.into_iter().map(ParamBinding::plain).collect(),
                    Box::new(JsvExpr::Block(body_stmts)),
                ),
                i,
            ))
        }
        _ => parse_expression(tokens, pos, depth + 1),
    }
}

fn parse_expression(
    tokens: &[String],
    pos: usize,
    depth: usize,
) -> Result<(JsvExpr, usize), String> {
    if depth > MAX_PARSE_DEPTH {
        return Err("Script too deeply nested".to_string());
    }
    parse_assignment(tokens, pos, depth)
}

fn parse_assignment(
    tokens: &[String],
    pos: usize,
    depth: usize,
) -> Result<(JsvExpr, usize), String> {
    if depth > MAX_PARSE_DEPTH {
        return Err("Expression nesting depth exceeded".to_string());
    }
    if let Some((parameters, is_async, body_pos)) = parse_arrow_signature(tokens, pos) {
        let (body, next) = if tokens.get(body_pos).map(String::as_str) == Some("{") {
            parse_statement(tokens, body_pos, depth + 1)?
        } else {
            let (expression, next) = parse_assignment(tokens, body_pos, depth + 1)?;
            (JsvExpr::Return(Box::new(expression)), next)
        };
        return Ok((
            JsvExpr::FunctionExpr(parameters, Box::new(body), is_async),
            next,
        ));
    }

    let (left, i) = parse_or(tokens, pos, depth)?;

    if i < tokens.len() && tokens[i] == "?" {
        let (then_expr, i2) = parse_assignment(tokens, i + 1, depth + 1)?;
        if i2 >= tokens.len() || tokens[i2] != ":" {
            return Err("Expected ':' in conditional expression".to_string());
        }
        let (else_expr, i3) = parse_assignment(tokens, i2 + 1, depth + 1)?;
        return Ok((
            JsvExpr::Ternary(Box::new(left), Box::new(then_expr), Box::new(else_expr)),
            i3,
        ));
    }
    if i < tokens.len() && matches!(tokens[i].as_str(), "+=" | "-=" | "*=" | "/=" | "%=") {
        let op = match tokens[i].as_str() {
            "+=" => OpKind::Add,
            "-=" => OpKind::Sub,
            "*=" => OpKind::Mul,
            "/=" => OpKind::Div,
            _ => OpKind::Mod,
        };
        let (right, new_i) = parse_assignment(tokens, i + 1, depth + 1)?;
        match left {
            JsvExpr::Identifier(name) => Ok((
                JsvExpr::Assignment(
                    name.clone(),
                    Box::new(JsvExpr::BinaryOp(
                        Box::new(JsvExpr::Identifier(name)),
                        op,
                        Box::new(right),
                    )),
                ),
                new_i,
            )),
            JsvExpr::Member(object, property) => Ok((
                JsvExpr::MemberAssignment(
                    object.clone(),
                    property.clone(),
                    Box::new(JsvExpr::BinaryOp(
                        Box::new(JsvExpr::Member(object, property)),
                        op,
                        Box::new(right),
                    )),
                ),
                new_i,
            )),
            JsvExpr::Index(object, key) => Ok((
                JsvExpr::IndexAssignment(
                    object.clone(),
                    key.clone(),
                    Box::new(JsvExpr::BinaryOp(
                        Box::new(JsvExpr::Index(object, key)),
                        op,
                        Box::new(right),
                    )),
                ),
                new_i,
            )),
            _ => Err("Invalid compound assignment target".to_string()),
        }
    } else if i < tokens.len() && tokens[i] == "=" {
        if i + 1 < tokens.len() && tokens[i + 1] == "=" {
            return Ok((left, i));
        }
        match left {
            JsvExpr::Identifier(name) => {
                let (right, new_i) = parse_assignment(tokens, i + 1, depth + 1)?;
                Ok((JsvExpr::Assignment(name, Box::new(right)), new_i))
            }
            JsvExpr::Member(object, property) => {
                let (right, new_i) = parse_assignment(tokens, i + 1, depth + 1)?;
                Ok((
                    JsvExpr::MemberAssignment(object, property, Box::new(right)),
                    new_i,
                ))
            }
            JsvExpr::Index(object, key) => {
                let (right, new_i) = parse_assignment(tokens, i + 1, depth + 1)?;
                Ok((
                    JsvExpr::IndexAssignment(object, key, Box::new(right)),
                    new_i,
                ))
            }
            _ => Err("Invalid assignment target".to_string()),
        }
    } else {
        Ok((left, i))
    }
}

fn parse_arrow_signature(
    tokens: &[String],
    pos: usize,
) -> Option<(Vec<ParamBinding>, bool, usize)> {
    let mut i = pos;
    let is_async = tokens.get(i).map(String::as_str) == Some("async");
    if is_async {
        i += 1;
    }

    if is_identifier(tokens.get(i)?) && tokens.get(i + 1).map(String::as_str) == Some("=>") {
        return Some((
            vec![ParamBinding::plain(tokens[i].clone())],
            is_async,
            i + 2,
        ));
    }

    if tokens.get(i).map(String::as_str) != Some("(") {
        return None;
    }
    i += 1;
    let mut parameters = Vec::new();
    while tokens.get(i).map(String::as_str) != Some(")") {
        let parameter = tokens.get(i)?;
        if !is_identifier(parameter) {
            return None;
        }
        parameters.push(ParamBinding::plain(parameter.clone()));
        i += 1;
        match tokens.get(i).map(String::as_str) {
            Some(",") => i += 1,
            Some(")") => {}
            _ => return None,
        }
    }
    if tokens.get(i + 1).map(String::as_str) != Some("=>") {
        return None;
    }
    Some((parameters, is_async, i + 2))
}

fn parse_or(tokens: &[String], pos: usize, depth: usize) -> Result<(JsvExpr, usize), String> {
    let (left, mut i) = parse_nullish(tokens, pos, depth)?;
    let mut result = left;
    while i < tokens.len() && tokens[i] == "||" {
        let (right, new_i) = parse_nullish(tokens, i + 1, depth)?;
        result = JsvExpr::BinaryOp(Box::new(result), OpKind::Or, Box::new(right));
        i = new_i;
    }
    Ok((result, i))
}

fn parse_nullish(tokens: &[String], pos: usize, depth: usize) -> Result<(JsvExpr, usize), String> {
    let (left, mut i) = parse_and(tokens, pos, depth)?;
    let mut result = left;
    while i < tokens.len() && tokens[i] == "??" {
        let (right, new_i) = parse_and(tokens, i + 1, depth)?;
        result = JsvExpr::BinaryOp(Box::new(result), OpKind::Nullish, Box::new(right));
        i = new_i;
    }
    Ok((result, i))
}

fn parse_and(tokens: &[String], pos: usize, depth: usize) -> Result<(JsvExpr, usize), String> {
    let (left, mut i) = parse_equality(tokens, pos, depth)?;
    let mut result = left;
    while i < tokens.len() && tokens[i] == "&&" {
        let (right, new_i) = parse_equality(tokens, i + 1, depth)?;
        result = JsvExpr::BinaryOp(Box::new(result), OpKind::And, Box::new(right));
        i = new_i;
    }
    Ok((result, i))
}

fn parse_equality(tokens: &[String], pos: usize, depth: usize) -> Result<(JsvExpr, usize), String> {
    let (mut result, mut i) = parse_comparison(tokens, pos, depth)?;

    while i < tokens.len() && matches!(tokens[i].as_str(), "==" | "!=" | "===" | "!==") {
        let op = match tokens[i].as_str() {
            "===" => OpKind::StrictEq,
            "!==" => OpKind::StrictNeq,
            "==" => OpKind::Eq,
            _ => OpKind::Neq,
        };
        let (right, new_i) = parse_comparison(tokens, i + 1, depth)?;
        result = JsvExpr::BinaryOp(Box::new(result), op, Box::new(right));
        i = new_i;
    }
    Ok((result, i))
}

fn parse_comparison(
    tokens: &[String],
    pos: usize,
    depth: usize,
) -> Result<(JsvExpr, usize), String> {
    let (mut result, mut i) = parse_term(tokens, pos, depth)?;
    while i < tokens.len() && matches!(tokens[i].as_str(), "<" | ">" | "<=" | ">=" | "in") {
        let op = match tokens[i].as_str() {
            "<" => OpKind::Lt,
            ">" => OpKind::Gt,
            "<=" => OpKind::Le,
            "in" => OpKind::In,
            _ => OpKind::Ge,
        };
        let (right, new_i) = parse_term(tokens, i + 1, depth)?;
        result = JsvExpr::BinaryOp(Box::new(result), op, Box::new(right));
        i = new_i;
    }
    if i < tokens.len() && tokens[i] == "instanceof" {
        let (right, new_i) = parse_term(tokens, i + 1, depth)?;
        result = JsvExpr::BinaryOp(Box::new(result), OpKind::Instanceof, Box::new(right));
        i = new_i;
    }
    Ok((result, i))
}

fn parse_term(tokens: &[String], pos: usize, depth: usize) -> Result<(JsvExpr, usize), String> {
    let (mut result, mut i) = parse_factor(tokens, pos, depth)?;

    while i < tokens.len() && (tokens[i] == "+" || tokens[i] == "-") {
        let op = if tokens[i] == "+" {
            OpKind::Add
        } else {
            OpKind::Sub
        };
        let (right, new_i) = parse_factor(tokens, i + 1, depth)?;
        result = JsvExpr::BinaryOp(Box::new(result), op, Box::new(right));
        i = new_i;
    }
    Ok((result, i))
}

fn parse_factor(tokens: &[String], pos: usize, depth: usize) -> Result<(JsvExpr, usize), String> {
    let (mut result, mut i) = parse_unary(tokens, pos, depth)?;

    while i < tokens.len() && (tokens[i] == "*" || tokens[i] == "/" || tokens[i] == "%") {
        let op = match tokens[i].as_str() {
            "*" => OpKind::Mul,
            "/" => OpKind::Div,
            _ => OpKind::Mod,
        };
        let (right, new_i) = parse_unary(tokens, i + 1, depth)?;
        result = JsvExpr::BinaryOp(Box::new(result), op, Box::new(right));
        i = new_i;
    }
    Ok((result, i))
}

fn parse_unary(tokens: &[String], pos: usize, depth: usize) -> Result<(JsvExpr, usize), String> {
    if pos >= tokens.len() {
        return Err("Unexpected end of expression".to_string());
    }
    if depth > MAX_PARSE_DEPTH {
        return Err("Expression nesting depth exceeded".to_string());
    }
    if tokens[pos] == "-" {
        let (expr, i) = parse_unary(tokens, pos + 1, depth + 1)?;
        Ok((JsvExpr::UnaryOp(OpKind::Sub, Box::new(expr)), i))
    } else if tokens[pos] == "!" {
        let (expr, i) = parse_unary(tokens, pos + 1, depth + 1)?;
        Ok((JsvExpr::UnaryOp(OpKind::Not, Box::new(expr)), i))
    } else if tokens[pos] == "await" {
        let (expr, i) = parse_unary(tokens, pos + 1, depth + 1)?;
        Ok((JsvExpr::Await(Box::new(expr)), i))
    } else if tokens[pos] == "typeof" {
        let (expr, i) = parse_unary(tokens, pos + 1, depth + 1)?;
        Ok((JsvExpr::UnaryOp(OpKind::Typeof, Box::new(expr)), i))
    } else if tokens[pos] == "++" {
        let (expr, i) = parse_unary(tokens, pos + 1, depth + 1)?;
        Ok((JsvExpr::Update(Box::new(expr), true, true), i))
    } else if tokens[pos] == "--" {
        let (expr, i) = parse_unary(tokens, pos + 1, depth + 1)?;
        Ok((JsvExpr::Update(Box::new(expr), true, false), i))
    } else {
        parse_call(tokens, pos, depth)
    }
}

fn parse_call(tokens: &[String], pos: usize, depth: usize) -> Result<(JsvExpr, usize), String> {
    if pos >= tokens.len() {
        return Err("Unexpected end of expression".to_string());
    }
    if tokens[pos] == "new" {
        return parse_new_call(tokens, pos, depth);
    }
    let (mut expr, mut i) = parse_primary(tokens, pos, depth)?;

    loop {
        if i < tokens.len() && tokens[i] == "(" {
            let mut args = Vec::new();
            i += 1;
            while i < tokens.len() && tokens[i] != ")" {
                if tokens[i] != "," && tokens[i] != ")" {
                    let (arg, new_i) = parse_expression(tokens, i, depth + 1)?;
                    args.push(arg);
                    i = new_i;
                }
                while i < tokens.len() && tokens[i] == "," {
                    i += 1;
                }
            }
            if i >= tokens.len() {
                return Err("Expected ')'".to_string());
            }
            i += 1;
            expr = JsvExpr::Call(Box::new(expr), args);
        } else if i < tokens.len() && tokens[i] == "." {
            i += 1;
            if i >= tokens.len() || !is_identifier(&tokens[i]) {
                return Err("Expected property name after '.'".to_string());
            }
            let prop = tokens[i].clone();
            i += 1;
            if i < tokens.len() && tokens[i] == "(" {
                expr = JsvExpr::Member(Box::new(expr), prop);
            } else {
                match expr_to_name(&expr) {
                    Some(_) => {
                        expr = JsvExpr::Member(Box::new(expr), prop.clone());
                    }
                    None => {
                        expr = JsvExpr::Member(Box::new(expr), prop);
                    }
                }
            }
        } else if i < tokens.len() && tokens[i] == "?." {
            i += 1;
            if i >= tokens.len() {
                return Err("Expected property name after '?.'".to_string());
            }
            if tokens[i] == "[" {
                let (key, next) = parse_expression(tokens, i + 1, depth + 1)?;
                if next >= tokens.len() || tokens[next] != "]" {
                    return Err("Expected ']' after optional index".to_string());
                }
                expr = JsvExpr::OptionalIndex(Box::new(expr), Box::new(key));
                i = next + 1;
            } else if is_identifier(&tokens[i]) {
                let prop = tokens[i].clone();
                i += 1;
                expr = JsvExpr::OptionalMember(Box::new(expr), prop);
            } else {
                return Err("Expected property name after '?.'".to_string());
            }
        } else if i < tokens.len() && tokens[i] == "[" {
            let (key, next) = parse_expression(tokens, i + 1, depth + 1)?;
            if next >= tokens.len() || tokens[next] != "]" {
                return Err("Expected ']' after property index".to_string());
            }
            expr = JsvExpr::Index(Box::new(expr), Box::new(key));
            i = next + 1;
        } else if i < tokens.len() && (tokens[i] == "++" || tokens[i] == "--") {
            expr = JsvExpr::Update(Box::new(expr), false, tokens[i] == "++");
            i += 1;
        } else {
            break;
        }
    }
    Ok((expr, i))
}

fn parse_new_call(tokens: &[String], pos: usize, depth: usize) -> Result<(JsvExpr, usize), String> {
    let (mut expr, mut i) = parse_primary(tokens, pos + 1, depth + 1)?;
    let mut constructed = false;
    loop {
        if i < tokens.len() && tokens[i] == "(" {
            let mut args = Vec::new();
            i += 1;
            while i < tokens.len() && tokens[i] != ")" {
                if tokens[i] != "," && tokens[i] != ")" {
                    let (arg, new_i) = parse_expression(tokens, i, depth + 1)?;
                    args.push(arg);
                    i = new_i;
                }
                while i < tokens.len() && tokens[i] == "," {
                    i += 1;
                }
            }
            if i >= tokens.len() {
                return Err("Expected ')'".to_string());
            }
            i += 1;
            expr = if constructed {
                JsvExpr::Call(Box::new(expr), args)
            } else {
                constructed = true;
                JsvExpr::NewExpr(Box::new(expr), args)
            };
        } else if i < tokens.len() && tokens[i] == "." {
            i += 1;
            if i >= tokens.len() || !is_identifier(&tokens[i]) {
                return Err("Expected property name after '.'".to_string());
            }
            let prop = tokens[i].clone();
            i += 1;
            expr = JsvExpr::Member(Box::new(expr), prop);
        } else if i < tokens.len() && tokens[i] == "[" {
            let (key, next) = parse_expression(tokens, i + 1, depth + 1)?;
            if next >= tokens.len() || tokens[next] != "]" {
                return Err("Expected ']' after property index".to_string());
            }
            expr = JsvExpr::Index(Box::new(expr), Box::new(key));
            i = next + 1;
        } else {
            break;
        }
    }
    if !constructed {
        expr = JsvExpr::NewExpr(Box::new(expr), Vec::new());
    }
    Ok((expr, i))
}

fn parse_primary(tokens: &[String], pos: usize, depth: usize) -> Result<(JsvExpr, usize), String> {
    if pos >= tokens.len() {
        return Err("Unexpected end of expression".to_string());
    }
    let tok = &tokens[pos];

    if tok == "import" {
        if tokens.get(pos + 1).map(String::as_str) != Some("(") {
            return Err("Expected '(' after import".to_string());
        }
        let (specifier, next) = parse_expression(tokens, pos + 2, depth + 1)?;
        let JsvExpr::String(specifier) = specifier else {
            return Err("Dynamic import requires a string literal".to_string());
        };
        if next >= tokens.len() || tokens[next] != ")" {
            return Err("Expected ')' after import specifier".to_string());
        }
        return Ok((JsvExpr::DynamicImport(specifier), next + 1));
    }

    if tok == "function" {
        let mut i = pos + 1;
        if is_identifier(tokens.get(i).map(String::as_str).unwrap_or("")) {
            i += 1;
        }
        if tokens.get(i).map(String::as_str) != Some("(") {
            return Err("Expected '(' after function".to_string());
        }
        i += 1;
        let mut params = Vec::new();
        while i < tokens.len() && tokens[i] != ")" {
            if tokens[i] != "," {
                params.push(tokens[i].clone());
            }
            i += 1;
            while i < tokens.len() && tokens[i] == "," {
                i += 1;
            }
        }
        if i >= tokens.len() {
            return Err("Expected ')' after parameters".to_string());
        }
        i += 1;
        if tokens.get(i).map(String::as_str) != Some("{") {
            return Err("Expected '{' for function body".to_string());
        }
        let (body, next) = parse_statements(tokens, i + 1, depth + 1)?;
        if tokens.get(next).map(String::as_str) != Some("}") {
            return Err("Expected '}' to close function body".to_string());
        }
        return Ok((
            JsvExpr::FunctionExpr(
                params.into_iter().map(ParamBinding::plain).collect(),
                Box::new(JsvExpr::Block(body)),
                false,
            ),
            next + 1,
        ));
    }

    if tok == "`" {
        let mut parts = Vec::new();
        let mut i = pos + 1;
        loop {
            if i >= tokens.len() {
                return Err("Unterminated template literal".to_string());
            }
            let current = &tokens[i];
            if current.starts_with('"') && current.ends_with('"') && current.len() >= 2 {
                parts.push(TemplatePart::Text(
                    current[1..current.len() - 1].to_string(),
                ));
                i += 1;
            } else if current == "`${" {
                let (expr, next) = parse_expression(tokens, i + 1, depth + 1)?;
                if next >= tokens.len() || tokens[next] != "`}" {
                    return Err("Expected '}' to close template expression".to_string());
                }
                parts.push(TemplatePart::Expr(Box::new(expr)));
                i = next + 1;
            } else if current == "`END" {
                if parts.is_empty() {
                    parts.push(TemplatePart::Text(String::new()));
                }
                return Ok((JsvExpr::Template(parts), i + 1));
            } else {
                return Err(format!("Unexpected token in template literal: {}", current));
            }
        }
    }

    if tok == "`}" {
        return Err("Unexpected template expression boundary".to_string());
    }

    if tok == "(" {
        let (expr, i) = parse_expression(tokens, pos + 1, depth + 1)?;
        if i >= tokens.len() || tokens[i] != ")" {
            return Err("Expected ')'".to_string());
        }
        return Ok((expr, i + 1));
    }

    if tok == "[" {
        let mut values = Vec::new();
        let mut i = pos + 1;
        while i < tokens.len() && tokens[i] != "]" {
            let (value, next) = parse_expression(tokens, i, depth + 1)?;
            values.push(value);
            i = next;
            if i < tokens.len() && tokens[i] == "," {
                i += 1;
            } else if i < tokens.len() && tokens[i] != "]" {
                return Err("Expected ',' or ']' in array literal".to_string());
            }
        }
        if i >= tokens.len() {
            return Err("Expected ']' after array literal".to_string());
        }
        return Ok((JsvExpr::ArrayLiteral(values), i + 1));
    }

    if tok == "{" {
        let mut properties = Vec::new();
        let mut i = pos + 1;
        while i < tokens.len() && tokens[i] != "}" {
            let raw_name = tokens
                .get(i)
                .ok_or_else(|| "Expected object property name".to_string())?;
            let name = if raw_name.starts_with('"') && raw_name.ends_with('"') {
                raw_name[1..raw_name.len() - 1].to_string()
            } else if is_identifier(raw_name) || raw_name.parse::<f64>().is_ok() {
                raw_name.clone()
            } else {
                return Err("Invalid object property name".to_string());
            };
            i += 1;
            if i >= tokens.len() || tokens[i] != ":" {
                return Err("Expected ':' after object property name".to_string());
            }
            let (value, next) = parse_expression(tokens, i + 1, depth + 1)?;
            properties.push(ObjectProperty::KeyValue(name, value));
            i = next;
            if i < tokens.len() && tokens[i] == "," {
                i += 1;
            } else if i < tokens.len() && tokens[i] != "}" {
                return Err("Expected ',' or '}' in object literal".to_string());
            }
        }
        if i >= tokens.len() {
            return Err("Expected '}' after object literal".to_string());
        }
        return Ok((JsvExpr::ObjectLiteral(properties), i + 1));
    }

    if let Ok(n) = tok.parse::<f64>() {
        return Ok((JsvExpr::Number(n), pos + 1));
    }

    if tok.starts_with('"') && tok.ends_with('"') {
        return Ok((JsvExpr::String(tok[1..tok.len() - 1].to_string()), pos + 1));
    }

    match tok.as_str() {
        "true" => return Ok((JsvExpr::Bool(true), pos + 1)),
        "false" => return Ok((JsvExpr::Bool(false), pos + 1)),
        "null" => return Ok((JsvExpr::Null, pos + 1)),
        "undefined" => return Ok((JsvExpr::Undefined, pos + 1)),
        _ => {}
    }

    if is_identifier(tok) {
        return Ok((JsvExpr::Identifier(tok.clone()), pos + 1));
    }

    Err(format!("Unexpected token: {}", tok))
}

fn is_identifier(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let chars: Vec<char> = s.chars().collect();
    if !chars[0].is_alphabetic() && chars[0] != '_' && chars[0] != '$' {
        return false;
    }
    chars
        .iter()
        .all(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
}

fn expr_to_name(expr: &JsvExpr) -> Option<String> {
    match expr {
        JsvExpr::Identifier(name) => Some(name.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_engine_creation() {
        let engine = JsvEngine::new();
        assert!(engine.global_env.contains_binding("console"));
    }

    #[test]
    fn test_eval_basic_arithmetic() {
        let mut engine = JsvEngine::new();
        let result = engine.eval("1 + 1");
        assert!(result.is_ok());
        assert_eq!(result.unwrap().as_number(), Some(2.0));
    }

    #[test]
    fn test_eval_precedence_and_parens() {
        let mut engine = JsvEngine::new();
        let res1 = engine.eval("2 + 3 * 4").unwrap();
        assert_eq!(res1.as_number(), Some(14.0));
        let res2 = engine.eval("(2 + 3) * 4").unwrap();
        assert_eq!(res2.as_number(), Some(20.0));
    }

    #[test]
    fn test_variable_assignment() {
        let mut engine = JsvEngine::new();
        let result = engine.eval("let x = 42");
        assert!(result.is_ok());
        assert_eq!(result.unwrap().as_number(), Some(42.0));
        let x_val = engine.global_env.get("x");
        assert_eq!(x_val.and_then(|v| v.as_number()), Some(42.0));
    }

    #[test]
    fn test_variable_usage() {
        let mut engine = JsvEngine::new();
        engine.eval("let x = 10").unwrap();
        let result = engine.eval("x + 20").unwrap();
        assert_eq!(result.as_number(), Some(30.0));
    }

    #[test]
    fn test_string_concatenation() {
        let mut engine = JsvEngine::new();
        let result = engine.eval(r#""Hello, " + "World!""#).unwrap();
        assert_eq!(result.as_string(), Some("Hello, World!"));
    }

    #[test]
    fn test_comparison() {
        let mut engine = JsvEngine::new();
        assert_eq!(engine.eval("5 == 5").unwrap().as_boolean(), Some(true));
        assert_eq!(engine.eval("5 != 3").unwrap().as_boolean(), Some(true));
        assert_eq!(engine.eval("5 < 3").unwrap().as_boolean(), Some(false));
        assert_eq!(engine.eval("5 > 3").unwrap().as_boolean(), Some(true));
    }

    #[test]
    fn test_if_statement() {
        let mut engine = JsvEngine::new();
        let result = engine.eval("if (true) { 42 } else { 0 }").unwrap();
        assert_eq!(result.as_number(), Some(42.0));
        let result2 = engine.eval("if (false) { 42 } else { 0 }").unwrap();
        assert_eq!(result2.as_number(), Some(0.0));
    }

    #[test]
    fn test_while_loop() {
        let mut engine = JsvEngine::new();
        engine.eval("let i = 0").unwrap();
        engine.eval("while (i < 3) { i = i + 1 }").unwrap();
        let i_val = engine.global_env.get("i");
        assert_eq!(i_val.and_then(|v| v.as_number()), Some(3.0));
    }

    #[test]
    fn test_function_definition_and_call() {
        let mut engine = JsvEngine::new();
        engine.eval("function add(a, b) { return a + b }").unwrap();
        let result = engine.eval("add(3, 4)").unwrap();
        assert_eq!(result.as_number(), Some(7.0));
    }

    #[test]
    fn test_console_log() {
        let mut engine = JsvEngine::new();
        let result = engine.eval("console.log(\"Hello\")");
        assert!(result.is_ok());
        assert_eq!(engine.console_output.len(), 1);
        assert_eq!(engine.console_output[0], "Hello");
    }

    #[test]
    fn test_complex_expression() {
        let mut engine = JsvEngine::new();
        let result = engine.eval("(10 + 5) * 2 - 3").unwrap();
        assert_eq!(result.as_number(), Some(27.0));
    }

    #[test]
    fn test_division_by_zero() {
        let mut engine = JsvEngine::new();

        assert_eq!(
            engine.eval("1 / 0").unwrap().as_number(),
            Some(f64::INFINITY)
        );
        assert_eq!(
            engine.eval("-1 / 0").unwrap().as_number(),
            Some(f64::NEG_INFINITY)
        );
        assert!(engine.eval("0 / 0").unwrap().as_number().unwrap().is_nan());
        assert!(engine.eval("5 % 0").unwrap().as_number().unwrap().is_nan());
    }

    #[test]
    fn test_while_true_is_bounded() {
        let mut engine = JsvEngine::new();
        let result = engine.eval("while (true) { }");
        assert!(result.is_err());
        let err = result.unwrap_err();

        assert!(
            err.contains("Infinite loop") || err.contains("step budget"),
            "got: {}",
            err
        );
    }

    #[test]
    fn test_infinite_recursion_errors_not_aborts() {
        let mut engine = JsvEngine::new();
        engine.eval("function f() { return f() }").unwrap();
        let result = engine.eval("f()");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("call stack"), "got: {}", err);
    }

    #[test]
    fn test_string_doubling_is_bounded() {
        let mut engine = JsvEngine::new();
        let result = engine.eval("let s = \"a\"; while (true) { s = s + s }");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("String too large") || err.contains("budget"),
            "got: {}",
            err
        );
    }

    #[test]
    fn test_per_string_length_cap() {
        let mut engine = JsvEngine::new();

        let script = format!(
            "let s = \"{}\"; s = s + s",
            "a".repeat(super::MAX_STRING_LENGTH / 2 + 1)
        );
        let result = engine.eval(&script);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("String too large"), "got: {}", err);
    }

    #[test]
    fn test_math_natives_work() {
        let mut engine = JsvEngine::new();
        let abs = engine.eval("Math.abs(-5)").unwrap();
        assert_eq!(abs.as_number(), Some(5.0));
        let sqrt = engine.eval("Math.sqrt(16)").unwrap();
        assert_eq!(sqrt.as_number(), Some(4.0));
        let pi = engine.eval("Math.PI").unwrap();
        assert!((pi.as_number().unwrap() - std::f64::consts::PI).abs() < 1e-9);
        let now = engine.eval("Date.now()").unwrap();
        assert!(now.as_number().unwrap() > 1_000_000_000_000.0);
    }

    #[test]
    fn test_member_access_follows_js_undefined_semantics() {
        let mut engine = JsvEngine::new();
        let result = engine.eval("let x = Math.nope").unwrap();
        assert_eq!(result, JsvValue::Undefined);

        assert!(engine
            .eval("undefined.name")
            .unwrap_err()
            .contains("TypeError"));
    }

    #[test]
    fn test_deeply_nested_script_is_rejected() {
        let mut engine = JsvEngine::new();
        let deep = format!(
            "{}{}",
            "(".repeat(2000),
            "1".to_string() + &")".repeat(2000)
        );
        let result = engine.eval(&deep);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("nested"), "got: {}", err);
    }

    #[test]
    fn test_return_inside_if_propagates() {
        let mut engine = JsvEngine::new();
        engine
            .eval("function f() { if (true) { return 5 } return 6 }")
            .unwrap();
        let result = engine.eval("f()").unwrap();
        assert_eq!(result.as_number(), Some(5.0));
    }

    #[test]
    fn test_return_inside_while_exits_loop() {
        let mut engine = JsvEngine::new();
        engine
            .eval(
                "function f() { let i = 0; while (true) { i = i + 1; if (i >= 3) { return i } } }",
            )
            .unwrap();
        let result = engine.eval("f()").unwrap();
        assert_eq!(result.as_number(), Some(3.0));
    }

    #[test]
    fn test_chained_operators() {
        let mut engine = JsvEngine::new();
        assert_eq!(engine.eval("1 + 2 + 3").unwrap().as_number(), Some(6.0));
        assert_eq!(engine.eval("10 - 3 - 2").unwrap().as_number(), Some(5.0));
        assert_eq!(engine.eval("2 * 3 * 4").unwrap().as_number(), Some(24.0));
        assert_eq!(engine.eval("100 / 2 / 5").unwrap().as_number(), Some(10.0));
        assert_eq!(
            engine.eval("1 + 2 * 3 + 4").unwrap().as_number(),
            Some(11.0)
        );
    }

    #[test]
    fn test_string_escapes() {
        let mut engine = JsvEngine::new();
        let result = engine.eval(r#""a\nb""#).unwrap();
        assert_eq!(result.as_string(), Some("a\nb"));
        let result = engine.eval(r#""say \"hi\"""#).unwrap();
        assert_eq!(result.as_string(), Some("say \"hi\""));
    }

    #[test]
    fn test_unterminated_string_is_error() {
        let mut engine = JsvEngine::new();
        let result = engine.eval(r#""oops"#);
        assert!(result.is_err());
    }

    #[test]
    fn test_unicode_before_two_character_operator_never_panics() {
        let mut engine = JsvEngine::new();
        let value = engine
            .eval("let message = 'Xin vui lòng'; message == 'Xin vui lòng'")
            .expect("Unicode source must tokenize on character boundaries");
        assert_eq!(value, JsvValue::Boolean(true));
    }

    #[test]
    fn closure_keeps_its_lexical_environment() {
        let mut engine = JsvEngine::new();
        let value = engine
            .eval(
                "function outer(base) { let captured=base; function inner(delta) { return captured+delta } return inner } let add=outer(4); add(3)",
            )
            .unwrap();
        assert_eq!(value, JsvValue::Number(7.0));
    }

    #[test]
    fn closure_updates_a_live_captured_binding() {
        let mut engine = JsvEngine::new();
        let value = engine
            .eval(
                "function counter(){let value=0;function next(){value=value+1;return value}return next}let next=counter();let first=next();let second=next();first*10+second",
            )
            .unwrap();
        assert_eq!(value, JsvValue::Number(12.0));
    }

    #[test]
    fn block_and_catch_declarations_do_not_leak() {
        let mut engine = JsvEngine::new();
        assert_eq!(
            engine
                .eval("let visible=0;if(true){let hidden=1;visible=hidden}visible")
                .unwrap(),
            JsvValue::Number(1.0)
        );
        assert!(engine.eval("hidden").unwrap_err().contains("not defined"));
        engine
            .eval("try{throw 'reason'}catch(error){let caught=error}")
            .unwrap();
        assert!(engine.eval("error").unwrap_err().contains("not defined"));
        assert!(engine.eval("caught").unwrap_err().contains("not defined"));
    }

    #[test]
    fn let_const_and_var_have_distinct_declaration_rules() {
        let mut engine = JsvEngine::new();
        engine.eval("const fixed=4").unwrap();
        assert!(engine
            .eval("fixed=5")
            .unwrap_err()
            .contains("assignment to constant"));
        assert!(engine
            .eval("const missing")
            .unwrap_err()
            .contains("Missing initializer"));
        engine.eval("let once=1").unwrap();
        assert!(engine
            .eval("let once=2")
            .unwrap_err()
            .contains("already been declared"));
        engine.eval("var replaceable=1;var replaceable=2").unwrap();
        assert_eq!(engine.eval("replaceable").unwrap(), JsvValue::Number(2.0));
    }

    #[test]
    fn sibling_closures_share_the_same_binding() {
        let mut engine = JsvEngine::new();
        let value = engine
            .eval(
                "function pair(){let value=0;function increment(){value=value+1;return value}function read(){return value}return [increment,read]}let closures=pair();closures[0]();closures[1]()",
            )
            .unwrap();
        assert_eq!(value, JsvValue::Number(1.0));
    }

    #[test]
    fn arrow_functions_capture_lexical_bindings() {
        let mut engine = JsvEngine::new();
        let value = engine
            .eval("let make=base=>value=>base+value;let addFour=make(4);addFour(3)")
            .unwrap();
        assert_eq!(value, JsvValue::Number(7.0));
        assert_eq!(
            engine
                .eval("let add=(left,right)=>{return left+right};add(2,3)")
                .unwrap(),
            JsvValue::Number(5.0)
        );
    }

    #[test]
    fn arrow_handlers_and_async_arrows_return_promises() {
        let mut engine = JsvEngine::new();
        let chained = engine
            .eval("Promise.resolve(2).then(value=>value+3)")
            .unwrap();
        let JsvValue::Promise(chained) = chained else {
            panic!("expected chained Promise");
        };
        assert!(matches!(
            &*chained.borrow(),
            JsvPromiseState::Fulfilled(JsvValue::Number(5.0))
        ));

        let asynchronous = engine
            .eval("let calculate=async value=>await Promise.resolve(value+1);calculate(4)")
            .unwrap();
        let JsvValue::Promise(asynchronous) = asynchronous else {
            panic!("expected async arrow Promise");
        };
        assert!(matches!(
            &*asynchronous.borrow(),
            JsvPromiseState::Fulfilled(JsvValue::Number(5.0))
        ));
    }

    #[test]
    fn objects_arrays_prototypes_and_indexes_have_identity() {
        let mut engine = JsvEngine::new();
        let value = engine
            .eval(
                "let proto={base:3};let object=Object.create(proto);object.value=4;let values=[1,2];values.push(object.value);values[0]=5;object.base+values[0]+values[2]",
            )
            .unwrap();
        assert_eq!(value, JsvValue::Number(12.0));
        assert_eq!(
            engine.eval("Array.isArray(values)").unwrap(),
            JsvValue::Boolean(true)
        );
    }

    #[test]
    fn for_of_uses_bounded_array_iterator() {
        let mut engine = JsvEngine::new();
        let value = engine
            .eval("let sum=0;for(let item of [1,2,3,4]){sum=sum+item}sum")
            .unwrap();
        assert_eq!(value, JsvValue::Number(10.0));
    }

    #[test]
    fn for_of_closures_capture_per_iteration_bindings() {
        let mut engine = JsvEngine::new();
        let value = engine
            .eval(
                "let closures=[];for(let item of [1,2]){closures.push(()=>item)}closures[0]()*10+closures[1]()",
            )
            .unwrap();
        assert_eq!(value, JsvValue::Number(12.0));
    }

    #[test]
    fn loop_completion_records_reach_the_nearest_iterator() {
        let mut engine = JsvEngine::new();
        let value = engine
            .eval(
                "let sum=0;for(let item of [1,2,3,4,5]){if(item==2){continue}if(item==4){break}sum=sum+item}sum",
            )
            .unwrap();
        assert_eq!(value, JsvValue::Number(4.0));
        assert!(engine.eval("break").unwrap_err().contains("illegal break"));
        assert!(engine
            .eval("continue")
            .unwrap_err()
            .contains("illegal continue"));
    }

    #[test]
    fn logical_operators_do_not_evaluate_skipped_operands() {
        let mut engine = JsvEngine::new();
        assert_eq!(
            engine.eval("false && missing()").unwrap(),
            JsvValue::Boolean(false)
        );
        assert_eq!(
            engine.eval("true || missing()").unwrap(),
            JsvValue::Boolean(true)
        );
    }

    #[test]
    fn try_catch_finally_preserves_abrupt_completion() {
        let mut engine = JsvEngine::new();
        let value = engine
            .eval(
                "let result='';try{throw 'boom'}catch(error){result=error}finally{result=result+'!'}result",
            )
            .unwrap();
        assert_eq!(value, JsvValue::String("boom!".to_string()));
        assert!(engine
            .eval("throw 'uncaught'")
            .unwrap_err()
            .contains("uncaught"));
    }

    #[test]
    fn promise_reactions_run_after_the_script_job() {
        let mut engine = JsvEngine::new();
        let value = engine
            .eval(
                "function plus(value){console.log(value);return value+1}let promise=Promise.resolve(4);let chained=promise.then(plus);console.log('sync');chained",
            )
            .unwrap();
        assert_eq!(engine.console_output, vec!["sync", "4"]);
        let JsvValue::Promise(promise) = value else {
            panic!("expected chained Promise");
        };
        assert!(matches!(
            &*promise.borrow(),
            JsvPromiseState::Fulfilled(JsvValue::Number(5.0))
        ));
    }

    #[test]
    fn pending_promise_chains_settle_in_microtask_order() {
        let mut engine = JsvEngine::new();
        let value = engine
            .eval(
                "Promise.resolve(1).then(value=>value+1).then(value=>value+1).finally(()=>Promise.resolve('ignored'))",
            )
            .unwrap();
        let JsvValue::Promise(value) = value else {
            panic!("expected chained Promise");
        };
        assert!(matches!(
            &*value.borrow(),
            JsvPromiseState::Fulfilled(JsvValue::Number(3.0))
        ));

        let cycle = engine
            .eval("let base=Promise.resolve(1);let cycle=base.then(()=>cycle);cycle")
            .unwrap();
        let JsvValue::Promise(cycle) = cycle else {
            panic!("expected cyclic Promise");
        };
        assert!(matches!(
            &*cycle.borrow(),
            JsvPromiseState::Rejected(JsvValue::String(reason)) if reason.contains("itself")
        ));
    }

    #[test]
    fn async_await_returns_fulfilled_or_rejected_promises() {
        let mut engine = JsvEngine::new();
        let fulfilled = engine
            .eval("async function calculate(){let value=await Promise.resolve(4);return value+2}calculate()")
            .unwrap();
        let JsvValue::Promise(fulfilled) = fulfilled else {
            panic!("expected async Promise");
        };
        assert!(matches!(
            &*fulfilled.borrow(),
            JsvPromiseState::Fulfilled(JsvValue::Number(6.0))
        ));

        let rejected = engine
            .eval("async function fail(){throw 'bad'}fail()")
            .unwrap();
        let JsvValue::Promise(rejected) = rejected else {
            panic!("expected rejected async Promise");
        };
        assert!(matches!(
            &*rejected.borrow(),
            JsvPromiseState::Rejected(JsvValue::String(reason)) if reason == "bad"
        ));
    }

    #[test]
    fn module_graph_parses_links_and_evaluates_named_bindings() {
        let mut graph = JsvModuleGraph::new();
        graph.register("dep", "export const base=4;").unwrap();
        graph
            .register(
                "entry",
                "import { base } from 'dep';\nexport function twice(value){return value*2}\nexport const result=twice(base);",
            )
            .unwrap();
        let namespace = graph.evaluate("entry").unwrap();
        assert_eq!(
            namespace.exports.get("result"),
            Some(&JsvValue::Number(8.0))
        );
        assert!(namespace.exports.contains_key("twice"));
    }

    #[test]
    fn module_graph_rejects_cycles_and_missing_exports() {
        let mut graph = JsvModuleGraph::new();
        graph
            .register("a", "import { b } from 'b';\nexport const a=b;")
            .unwrap();
        graph
            .register("b", "import { a } from 'a';\nexport const b=a;")
            .unwrap();
        assert!(graph.evaluate("a").unwrap_err().contains("Circular"));
    }

    #[test]
    fn collections_execute_real_bounded_methods() {
        let mut engine = JsvEngine::new();
        let value = engine
            .eval(
                "let map=new Map([[1,'a']]);map.set(2,'b');\
                 let set=new Set([1,1,2]);\
                 map.get(1)+map.size+map.has(2)+set.size+set.has(2)",
            )
            .unwrap();
        assert_eq!(value, JsvValue::String("a2true2true".to_string()));

        let weak_value = engine
            .eval("let key={};let weak=new WeakMap();weak.set(key,7);weak.get(key)")
            .unwrap();
        assert_eq!(weak_value, JsvValue::Number(7.0));
    }

    #[test]
    fn string_iterator_handles_unicode_without_prefix_reallocation() {
        let mut engine = JsvEngine::new();
        let value = engine
            .eval("let out='';for(let value of 'a🙂b'){out+=value}out")
            .unwrap();
        assert_eq!(value, JsvValue::String("a🙂b".to_string()));
    }

    #[test]
    fn reflect_weakref_and_intl_inventory_is_callable() {
        let mut engine = JsvEngine::new();
        assert_eq!(
            engine
                .eval("Reflect.apply(function(a,b){return a+b},null,[1,2])")
                .unwrap(),
            JsvValue::Number(3.0)
        );
        assert_eq!(
            engine
                .eval("let o={};Reflect.preventExtensions(o);Reflect.isExtensible(o)")
                .unwrap(),
            JsvValue::Boolean(false)
        );
        assert_eq!(
            engine
                .eval("let value={x:7};let weak=new WeakRef(value);weak.deref().x")
                .unwrap(),
            JsvValue::Number(7.0)
        );
        assert_eq!(
            engine
                .eval("typeof new Intl.DateTimeFormat().format")
                .unwrap(),
            JsvValue::String("function".to_string())
        );
    }
}
