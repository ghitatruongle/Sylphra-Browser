use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use crate::resource_caps;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SubsystemId {
    DomTree,
    LayoutTree,
    RuntimeHeap,
    ImageCache,
    ResourceCache,
    Media,
    Gpu,
    WorkerIpc,
    StorageWrite,
}

impl SubsystemId {
    pub const fn all() -> [SubsystemId; 9] {
        [
            SubsystemId::DomTree,
            SubsystemId::LayoutTree,
            SubsystemId::RuntimeHeap,
            SubsystemId::ImageCache,
            SubsystemId::ResourceCache,
            SubsystemId::Media,
            SubsystemId::Gpu,
            SubsystemId::WorkerIpc,
            SubsystemId::StorageWrite,
        ]
    }

    pub fn label(self) -> &'static str {
        match self {
            SubsystemId::DomTree => "dom",
            SubsystemId::LayoutTree => "layout",
            SubsystemId::RuntimeHeap => "runtime",
            SubsystemId::ImageCache => "images",
            SubsystemId::ResourceCache => "cache",
            SubsystemId::Media => "media",
            SubsystemId::Gpu => "gpu",
            SubsystemId::WorkerIpc => "ipc",
            SubsystemId::StorageWrite => "storage",
        }
    }

    pub fn default_ceiling_bytes(self) -> usize {
        match self {
            SubsystemId::DomTree => resource_caps::mb(resource_caps::SUBSYSTEM_DOM_TREE_MB),
            SubsystemId::LayoutTree => resource_caps::mb(resource_caps::SUBSYSTEM_LAYOUT_TREE_MB),
            SubsystemId::RuntimeHeap => resource_caps::mb(resource_caps::SUBSYSTEM_RUNTIME_HEAP_MB),
            SubsystemId::ImageCache => resource_caps::mb(resource_caps::SUBSYSTEM_IMAGE_CACHE_MB),
            SubsystemId::ResourceCache => {
                resource_caps::mb(resource_caps::SUBSYSTEM_RESOURCE_CACHE_MB)
            }
            SubsystemId::Media => resource_caps::mb(resource_caps::SUBSYSTEM_MEDIA_MB),
            SubsystemId::Gpu => resource_caps::mb(resource_caps::SUBSYSTEM_GPU_MB),
            SubsystemId::WorkerIpc => resource_caps::mb(resource_caps::SUBSYSTEM_WORKER_IPC_MB),
            SubsystemId::StorageWrite => {
                resource_caps::mb(resource_caps::SUBSYSTEM_STORAGE_WRITE_MB)
            }
        }
    }
}

impl fmt::Display for SubsystemId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum OwnerId {
    Global,
    Tab(usize),
    Process(u64),
}

impl OwnerId {
    pub fn label(self) -> String {
        match self {
            OwnerId::Global => "global".to_string(),
            OwnerId::Tab(id) => format!("tab:{id}"),
            OwnerId::Process(id) => format!("process:{id}"),
        }
    }
}

impl fmt::Display for OwnerId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.label())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LedgerLimits {
    pub soft_bytes: usize,
    pub hard_bytes: usize,
}

impl Default for LedgerLimits {
    fn default() -> Self {
        Self {
            soft_bytes: resource_caps::mb(resource_caps::GLOBAL_SOFT_LIMIT_MB),
            hard_bytes: resource_caps::mb(resource_caps::GLOBAL_HARD_LIMIT_MB),
        }
    }
}

impl LedgerLimits {
    pub fn from_mb(soft_mb: u32, hard_mb: u32) -> Self {
        let soft_bytes = resource_caps::mb(soft_mb);
        Self {
            soft_bytes,
            hard_bytes: resource_caps::mb(hard_mb).max(soft_bytes),
        }
    }

    pub fn byte_limits(soft_bytes: usize, hard_bytes: usize) -> Self {
        Self {
            soft_bytes: soft_bytes.min(hard_bytes),
            hard_bytes,
        }
    }

    pub fn unlimited() -> Self {
        Self {
            soft_bytes: usize::MAX,
            hard_bytes: usize::MAX,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DenyScope {
    SubsystemCeiling,
    GlobalHardLimit,
    UnknownSubsystem,
    DetachedLedger,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceDenied {
    pub subsystem: SubsystemId,
    pub owner: OwnerId,
    pub requested_bytes: usize,
    pub available_bytes: usize,
    pub ceiling_bytes: usize,
    pub scope: DenyScope,
}

impl ResourceDenied {
    pub fn message(&self) -> String {
        let reason = match self.scope {
            DenyScope::SubsystemCeiling => "subsystem ceiling",
            DenyScope::GlobalHardLimit => "global hard limit",
            DenyScope::UnknownSubsystem => "unregistered subsystem",
            DenyScope::DetachedLedger => "detached ledger",
        };
        format!(
            "{} reservation of {} bytes for {reason} denied ({} bytes available)",
            self.subsystem, self.requested_bytes, self.available_bytes
        )
    }
}

impl fmt::Display for ResourceDenied {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message())
    }
}

impl std::error::Error for ResourceDenied {}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LedgerTotals {
    pub committed_bytes: usize,
    pub reserved_bytes: usize,
    pub headroom_bytes: usize,
    pub denials: usize,
    pub denial_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubsystemUsage {
    pub subsystem: SubsystemId,
    pub ceiling_bytes: usize,
    pub committed_bytes: usize,
    pub reserved_bytes: usize,
    pub denials: usize,
}

impl SubsystemUsage {
    pub fn held_bytes(&self) -> usize {
        self.committed_bytes.saturating_add(self.reserved_bytes)
    }

    pub fn utilisation_percent(&self) -> u32 {
        if self.ceiling_bytes == 0 {
            return 0;
        }
        ((self.held_bytes().saturating_mul(100)) / self.ceiling_bytes).min(100) as u32
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerSnapshot {
    pub limits: LedgerLimits,
    pub totals: LedgerTotals,
    pub subsystems: Vec<SubsystemUsage>,
    pub real_working_set_bytes: u64,
    pub live_reservations: usize,
}

impl LedgerSnapshot {
    pub fn usage_of(&self, subsystem: SubsystemId) -> Option<SubsystemUsage> {
        self.subsystems
            .iter()
            .find(|usage| usage.subsystem == subsystem)
            .copied()
    }

    pub fn most_saturated_subsystem(&self) -> Option<SubsystemId> {
        self.subsystems
            .iter()
            .filter(|usage| usage.held_bytes() > 0)
            .max_by_key(|usage| usage.utilisation_percent())
            .map(|usage| usage.subsystem)
    }
}

#[derive(Debug)]
struct ReservationRecord {
    subsystem: SubsystemId,
    owner: OwnerId,
    committed_bytes: usize,
    outstanding_bytes: usize,
}

#[derive(Debug, Clone, Copy, Default)]
struct SubsystemCounters {
    ceiling_bytes: usize,
    committed_bytes: usize,
    reserved_bytes: usize,
    denials: usize,
}

impl SubsystemCounters {
    fn held_bytes(&self) -> usize {
        self.committed_bytes.saturating_add(self.reserved_bytes)
    }
}

#[derive(Debug)]
struct State {
    subsystems: HashMap<SubsystemId, SubsystemCounters>,
    records: HashMap<u64, ReservationRecord>,
    by_owner: HashMap<OwnerId, Vec<u64>>,
    reconciled: HashMap<(SubsystemId, OwnerId), u64>,
    committed_total: usize,
    reserved_total: usize,
    denial_count: usize,
    denial_bytes: usize,
    grant_count: usize,
    next_id: u64,
    main_working_set_bytes: u64,
    child_working_set_bytes: HashMap<u32, u64>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            subsystems: HashMap::new(),
            records: HashMap::new(),
            by_owner: HashMap::new(),
            reconciled: HashMap::new(),
            committed_total: 0,
            reserved_total: 0,
            denial_count: 0,
            denial_bytes: 0,
            grant_count: 0,
            next_id: 1,
            main_working_set_bytes: 0,
            child_working_set_bytes: HashMap::new(),
        }
    }
}

impl State {
    fn forget_reservation(&mut self, id: u64) {
        let Some(record) = self.records.remove(&id) else {
            return;
        };
        if let Some(ids) = self.by_owner.get_mut(&record.owner) {
            ids.retain(|existing| *existing != id);
            if ids.is_empty() {
                self.by_owner.remove(&record.owner);
            }
        }
    }
}

#[derive(Debug)]
pub struct ResourceLedger {
    limits: Mutex<LedgerLimits>,
    state: Mutex<State>,
}

pub type SharedLedger = Arc<ResourceLedger>;

static SHARED_LEDGER: OnceLock<SharedLedger> = OnceLock::new();

impl ResourceLedger {
    pub fn new(limits: LedgerLimits) -> SharedLedger {
        let ledger = Arc::new(Self {
            limits: Mutex::new(LedgerLimits {
                soft_bytes: limits.soft_bytes.min(limits.hard_bytes),
                hard_bytes: limits.hard_bytes,
            }),
            state: Mutex::new(State::default()),
        });
        ledger.register_default_subsystems();
        ledger
    }

    pub fn with_budget_mb(soft_mb: u32, hard_mb: u32) -> SharedLedger {
        Self::new(LedgerLimits::from_mb(soft_mb, hard_mb))
    }

    pub fn shared() -> SharedLedger {
        SHARED_LEDGER
            .get_or_init(|| Self::new(LedgerLimits::default()))
            .clone()
    }

    pub fn install_shared(ledger: SharedLedger) -> bool {
        SHARED_LEDGER.set(ledger).is_ok()
    }

    pub fn limits(&self) -> LedgerLimits {
        *self.lock_limits()
    }

    pub fn set_limits(&self, limits: LedgerLimits) {
        let mut guard = self.lock_limits();
        *guard = LedgerLimits {
            soft_bytes: limits.soft_bytes.min(limits.hard_bytes),
            hard_bytes: limits.hard_bytes,
        };
    }

    pub fn register_subsystem(&self, subsystem: SubsystemId, ceiling_bytes: usize) {
        let mut state = self.lock_state();
        let counters = state.subsystems.entry(subsystem).or_default();
        counters.ceiling_bytes = ceiling_bytes.max(counters.held_bytes());
    }

    pub fn register_default_subsystems(&self) {
        for subsystem in SubsystemId::all() {
            self.register_subsystem(subsystem, subsystem.default_ceiling_bytes());
        }
    }

    pub fn ceiling_bytes(&self, subsystem: SubsystemId) -> usize {
        self.lock_state()
            .subsystems
            .get(&subsystem)
            .map_or(0, |counters| counters.ceiling_bytes)
    }

    pub fn is_registered(&self, subsystem: SubsystemId) -> bool {
        self.lock_state().subsystems.contains_key(&subsystem)
    }

    pub fn reserve(
        self: &Arc<Self>,
        subsystem: SubsystemId,
        owner: OwnerId,
        bytes: usize,
    ) -> Result<Reservation, ResourceDenied> {
        if bytes == 0 {
            return Ok(Reservation::detached(subsystem, owner));
        }
        let limits = self.limits();
        let mut state = self.lock_state();
        let Some(counters) = state.subsystems.get(&subsystem) else {
            state.record_denial(subsystem, bytes);
            return Err(ResourceDenied {
                subsystem,
                owner,
                requested_bytes: bytes,
                available_bytes: 0,
                ceiling_bytes: 0,
                scope: DenyScope::UnknownSubsystem,
            });
        };
        if counters.held_bytes().saturating_add(bytes) > counters.ceiling_bytes {
            let available = counters.ceiling_bytes.saturating_sub(counters.held_bytes());
            let ceiling = counters.ceiling_bytes;
            state.record_denial(subsystem, bytes);
            return Err(ResourceDenied {
                subsystem,
                owner,
                requested_bytes: bytes,
                available_bytes: available,
                ceiling_bytes: ceiling,
                scope: DenyScope::SubsystemCeiling,
            });
        }
        let projected = state
            .committed_total
            .saturating_add(state.reserved_total)
            .saturating_add(bytes);
        if projected > limits.hard_bytes {
            let counters = state.subsystems.get(&subsystem).unwrap();
            let ceiling = counters.ceiling_bytes;
            let available = limits
                .hard_bytes
                .saturating_sub(projected.saturating_sub(bytes));
            state.record_denial(subsystem, bytes);
            return Err(ResourceDenied {
                subsystem,
                owner,
                requested_bytes: bytes,
                available_bytes: available,
                ceiling_bytes: ceiling,
                scope: DenyScope::GlobalHardLimit,
            });
        }
        let id = state.next_id;
        state.next_id = id.saturating_add(1);
        let counters = state.subsystems.get_mut(&subsystem).unwrap();
        counters.reserved_bytes = counters.reserved_bytes.saturating_add(bytes);
        state.reserved_total = state.reserved_total.saturating_add(bytes);
        state.grant_count = state.grant_count.saturating_add(1);
        state.records.insert(
            id,
            ReservationRecord {
                subsystem,
                owner,
                committed_bytes: 0,
                outstanding_bytes: bytes,
            },
        );
        state.by_owner.entry(owner).or_default().push(id);
        Ok(Reservation {
            ledger: Arc::downgrade(self),
            id,
            subsystem,
            owner,
            outstanding_bytes: bytes,
            committed_bytes: 0,
        })
    }

    pub fn lease(
        self: &Arc<Self>,
        subsystem: SubsystemId,
        owner: OwnerId,
        bytes: usize,
    ) -> Result<Lease, ResourceDenied> {
        let mut reservation = self.reserve(subsystem, owner, bytes)?;
        reservation.commit();
        Ok(Lease { reservation })
    }

    pub fn reconcile(
        self: &Arc<Self>,
        subsystem: SubsystemId,
        owner: OwnerId,
        target_bytes: usize,
    ) -> usize {
        let key = (subsystem, owner);
        let existing = self.lock_state().reconciled.get(&key).copied();
        let Some(id) = existing else {
            return self.reconcile_fresh(key, subsystem, owner, target_bytes);
        };
        let record_held = {
            let mut state = self.lock_state();
            match state.records.get(&id) {
                Some(record) => record
                    .committed_bytes
                    .saturating_add(record.outstanding_bytes),
                None => {
                    state.reconciled.remove(&key);
                    return self.reconcile_fresh(key, subsystem, owner, target_bytes);
                }
            }
        };
        if record_held == target_bytes {
            return record_held;
        }
        if record_held > target_bytes {
            self.apply_release(id, 0, record_held - target_bytes);
            if target_bytes == 0 {
                self.lock_state().reconciled.remove(&key);
            }
            return target_bytes;
        }
        match self.apply_grow(id, target_bytes - record_held) {
            Ok(held) => held,
            Err(denied) => {
                log::debug!("{}", denied.message());
                record_held
            }
        }
    }

    fn reconcile_fresh(
        self: &Arc<Self>,
        key: (SubsystemId, OwnerId),
        subsystem: SubsystemId,
        owner: OwnerId,
        target_bytes: usize,
    ) -> usize {
        if target_bytes == 0 {
            return 0;
        }
        match self.reserve(subsystem, owner, target_bytes) {
            Ok(mut reservation) => {
                let reservation_id = reservation.id;
                reservation.commit();
                std::mem::forget(reservation);
                self.lock_state().reconciled.insert(key, reservation_id);
                target_bytes
            }
            Err(denied) => {
                log::debug!("{}", denied.message());
                0
            }
        }
    }

    pub fn forget_reconciled(&self, subsystem: SubsystemId, owner: OwnerId) -> usize {
        let Some(id) = self.lock_state().reconciled.remove(&(subsystem, owner)) else {
            return 0;
        };
        let (outstanding, committed) =
            self.lock_state().records.get(&id).map_or((0, 0), |record| {
                (record.outstanding_bytes, record.committed_bytes)
            });
        self.apply_release(id, outstanding, committed);
        outstanding.saturating_add(committed)
    }

    fn apply_commit(&self, id: u64, bytes: usize) {
        if bytes == 0 {
            return;
        }
        let mut state = self.lock_state();
        let Some(subsystem) = state.records.get(&id).map(|record| record.subsystem) else {
            return;
        };
        let Some(record) = state.records.get_mut(&id) else {
            return;
        };
        let moving = bytes.min(record.outstanding_bytes);
        record.outstanding_bytes -= moving;
        record.committed_bytes = record.committed_bytes.saturating_add(moving);
        if let Some(counters) = state.subsystems.get_mut(&subsystem) {
            counters.reserved_bytes = counters.reserved_bytes.saturating_sub(moving);
            counters.committed_bytes = counters.committed_bytes.saturating_add(moving);
        }
        state.reserved_total = state.reserved_total.saturating_sub(moving);
        state.committed_total = state.committed_total.saturating_add(moving);
    }

    fn apply_grow(&self, id: u64, bytes: usize) -> Result<usize, ResourceDenied> {
        let limits = self.limits();
        let mut state = self.lock_state();
        let (subsystem, owner) = match state.records.get(&id) {
            Some(record) => (record.subsystem, record.owner),
            None => {
                return Err(ResourceDenied {
                    subsystem: SubsystemId::DomTree,
                    owner: OwnerId::Global,
                    requested_bytes: bytes,
                    available_bytes: 0,
                    ceiling_bytes: 0,
                    scope: DenyScope::DetachedLedger,
                })
            }
        };
        let counters = match state.subsystems.get(&subsystem) {
            Some(counters) => *counters,
            None => {
                state.record_denial(subsystem, bytes);
                return Err(ResourceDenied {
                    subsystem,
                    owner,
                    requested_bytes: bytes,
                    available_bytes: 0,
                    ceiling_bytes: 0,
                    scope: DenyScope::UnknownSubsystem,
                });
            }
        };
        if counters.held_bytes().saturating_add(bytes) > counters.ceiling_bytes {
            let available = counters.ceiling_bytes.saturating_sub(counters.held_bytes());
            let ceiling = counters.ceiling_bytes;
            state.record_denial(subsystem, bytes);
            return Err(ResourceDenied {
                subsystem,
                owner,
                requested_bytes: bytes,
                available_bytes: available,
                ceiling_bytes: ceiling,
                scope: DenyScope::SubsystemCeiling,
            });
        }
        let projected = state
            .committed_total
            .saturating_add(state.reserved_total)
            .saturating_add(bytes);
        if projected > limits.hard_bytes {
            let available = limits
                .hard_bytes
                .saturating_sub(projected.saturating_sub(bytes));
            state.record_denial(subsystem, bytes);
            return Err(ResourceDenied {
                subsystem,
                owner,
                requested_bytes: bytes,
                available_bytes: available,
                ceiling_bytes: limits.hard_bytes,
                scope: DenyScope::GlobalHardLimit,
            });
        }
        let counters = state.subsystems.get_mut(&subsystem).unwrap();
        counters.committed_bytes = counters.committed_bytes.saturating_add(bytes);
        if let Some(record) = state.records.get_mut(&id) {
            record.committed_bytes = record.committed_bytes.saturating_add(bytes);
        }
        state.committed_total = state.committed_total.saturating_add(bytes);
        state.grant_count = state.grant_count.saturating_add(1);
        Ok(state.records.get(&id).map_or(0, |record| {
            record
                .committed_bytes
                .saturating_add(record.outstanding_bytes)
        }))
    }

    fn apply_release(&self, id: u64, outstanding: usize, committed: usize) {
        let mut state = self.lock_state();
        let Some(subsystem) = state.records.get(&id).map(|record| record.subsystem) else {
            return;
        };
        let (releasing, returning, exhausted) = {
            let Some(record) = state.records.get_mut(&id) else {
                return;
            };
            let releasing = outstanding.min(record.outstanding_bytes);
            let returning = committed.min(record.committed_bytes);
            record.outstanding_bytes -= releasing;
            record.committed_bytes -= returning;
            let exhausted = record.outstanding_bytes == 0 && record.committed_bytes == 0;
            (releasing, returning, exhausted)
        };
        if let Some(counters) = state.subsystems.get_mut(&subsystem) {
            counters.reserved_bytes = counters.reserved_bytes.saturating_sub(releasing);
            counters.committed_bytes = counters.committed_bytes.saturating_sub(returning);
        }
        state.reserved_total = state.reserved_total.saturating_sub(releasing);
        state.committed_total = state.committed_total.saturating_sub(returning);
        if exhausted {
            state.forget_reservation(id);
        }
    }

    pub fn release_owner(&self, owner: OwnerId) -> usize {
        let mut state = self.lock_state();
        let reconciled: Vec<(SubsystemId, OwnerId)> = state
            .reconciled
            .iter()
            .filter(|(_, id)| {
                state
                    .records
                    .get(*id)
                    .map_or(true, |record| record.owner == owner)
            })
            .map(|(key, _)| *key)
            .collect();
        for key in reconciled {
            state.reconciled.remove(&key);
        }
        let ids = state.by_owner.remove(&owner).unwrap_or_default();
        let mut freed = 0usize;
        for id in ids {
            if let Some(record) = state.records.remove(&id) {
                state.reserved_total = state
                    .reserved_total
                    .saturating_sub(record.outstanding_bytes);
                state.committed_total =
                    state.committed_total.saturating_sub(record.committed_bytes);
                if let Some(counters) = state.subsystems.get_mut(&record.subsystem) {
                    counters.reserved_bytes = counters
                        .reserved_bytes
                        .saturating_sub(record.outstanding_bytes);
                    counters.committed_bytes = counters
                        .committed_bytes
                        .saturating_sub(record.committed_bytes);
                }
                freed = freed
                    .saturating_add(record.outstanding_bytes)
                    .saturating_add(record.committed_bytes);
            }
        }
        freed
    }

    pub fn note_main_working_set(&self, bytes: u64) {
        self.lock_state().main_working_set_bytes = bytes;
    }

    pub fn note_child_working_set(&self, os_process_id: u32, bytes: u64) {
        let mut state = self.lock_state();
        if bytes == 0 {
            state.child_working_set_bytes.remove(&os_process_id);
        } else {
            state.child_working_set_bytes.insert(os_process_id, bytes);
        }
    }

    pub fn forget_child(&self, os_process_id: u32) {
        self.lock_state()
            .child_working_set_bytes
            .remove(&os_process_id);
    }

    pub fn real_working_set_bytes(&self) -> u64 {
        let state = self.lock_state();
        state
            .main_working_set_bytes
            .saturating_add(state.child_working_set_bytes.values().sum::<u64>())
    }

    pub fn main_working_set_bytes(&self) -> u64 {
        self.lock_state().main_working_set_bytes
    }

    pub fn child_working_set_bytes(&self) -> u64 {
        self.lock_state().child_working_set_bytes.values().sum()
    }

    pub fn child_process_count(&self) -> usize {
        self.lock_state().child_working_set_bytes.len()
    }

    pub fn aggregate_bytes(&self) -> usize {
        let state = self.lock_state();
        let real = state
            .main_working_set_bytes
            .saturating_add(state.child_working_set_bytes.values().sum::<u64>());
        state.committed_total.max(real as usize)
    }

    pub fn owner_bytes(&self, owner: OwnerId) -> usize {
        let state = self.lock_state();
        state
            .by_owner
            .get(&owner)
            .map(|ids| {
                ids.iter().fold(0usize, |total, id| {
                    total.saturating_add(state.records.get(id).map_or(0, |record| {
                        record
                            .committed_bytes
                            .saturating_add(record.outstanding_bytes)
                    }))
                })
            })
            .unwrap_or_default()
    }

    pub fn reservations_for_owner(&self, owner: OwnerId) -> Vec<u64> {
        self.lock_state()
            .by_owner
            .get(&owner)
            .cloned()
            .unwrap_or_default()
    }

    pub fn totals(&self) -> LedgerTotals {
        let limits = self.limits();
        let state = self.lock_state();
        let held = state.committed_total.saturating_add(state.reserved_total);
        LedgerTotals {
            committed_bytes: state.committed_total,
            reserved_bytes: state.reserved_total,
            headroom_bytes: limits.hard_bytes.saturating_sub(held),
            denials: state.denial_count,
            denial_bytes: state.denial_bytes,
        }
    }

    pub fn snapshot(&self) -> LedgerSnapshot {
        let limits = self.limits();
        let state = self.lock_state();
        let subsystems = SubsystemId::all()
            .into_iter()
            .map(|subsystem| {
                let counters = state.subsystems.get(&subsystem);
                SubsystemUsage {
                    subsystem,
                    ceiling_bytes: counters.map_or(0, |counters| counters.ceiling_bytes),
                    committed_bytes: counters.map_or(0, |counters| counters.committed_bytes),
                    reserved_bytes: counters.map_or(0, |counters| counters.reserved_bytes),
                    denials: counters.map_or(0, |counters| counters.denials),
                }
            })
            .collect();
        let held = state.committed_total.saturating_add(state.reserved_total);
        LedgerSnapshot {
            limits,
            totals: LedgerTotals {
                committed_bytes: state.committed_total,
                reserved_bytes: state.reserved_total,
                headroom_bytes: limits.hard_bytes.saturating_sub(held),
                denials: state.denial_count,
                denial_bytes: state.denial_bytes,
            },
            subsystems,
            real_working_set_bytes: state
                .main_working_set_bytes
                .saturating_add(state.child_working_set_bytes.values().sum::<u64>()),
            live_reservations: state.records.len(),
        }
    }

    pub fn grant_count(&self) -> usize {
        self.lock_state().grant_count
    }

    pub fn clear_all(&self) {
        let mut state = self.lock_state();
        *state = State::default();
        state.register_defaults();
    }

    fn lock_limits(&self) -> std::sync::MutexGuard<'_, LedgerLimits> {
        self.limits
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl State {
    fn record_denial(&mut self, subsystem: SubsystemId, bytes: usize) {
        self.denial_count = self.denial_count.saturating_add(1);
        self.denial_bytes = self.denial_bytes.saturating_add(bytes);
        if let Some(counters) = self.subsystems.get_mut(&subsystem) {
            counters.denials = counters.denials.saturating_add(1);
        }
    }

    fn register_defaults(&mut self) {
        for subsystem in SubsystemId::all() {
            let ceiling = subsystem.default_ceiling_bytes();
            self.subsystems.entry(subsystem).or_default().ceiling_bytes = ceiling;
        }
    }
}

#[derive(Debug)]
pub struct Reservation {
    ledger: Weak<ResourceLedger>,
    id: u64,
    subsystem: SubsystemId,
    owner: OwnerId,
    outstanding_bytes: usize,
    committed_bytes: usize,
}

impl Reservation {
    fn detached(subsystem: SubsystemId, owner: OwnerId) -> Self {
        Self {
            ledger: Weak::new(),
            id: 0,
            subsystem,
            owner,
            outstanding_bytes: 0,
            committed_bytes: 0,
        }
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn subsystem(&self) -> SubsystemId {
        self.subsystem
    }

    pub fn owner(&self) -> OwnerId {
        self.owner
    }

    pub fn outstanding_bytes(&self) -> usize {
        self.outstanding_bytes
    }

    pub fn committed_bytes(&self) -> usize {
        self.committed_bytes
    }

    pub fn held_bytes(&self) -> usize {
        self.outstanding_bytes.saturating_add(self.committed_bytes)
    }

    pub fn is_detached(&self) -> bool {
        self.ledger.upgrade().is_none()
    }

    pub fn commit(&mut self) -> usize {
        let moving = self.outstanding_bytes;
        self.outstanding_bytes = 0;
        self.committed_bytes = self.committed_bytes.saturating_add(moving);
        if let Some(ledger) = self.ledger.upgrade() {
            ledger.apply_commit(self.id, moving);
        }
        self.committed_bytes
    }

    pub fn commit_bytes(&mut self, bytes: usize) -> usize {
        let moving = bytes.min(self.outstanding_bytes);
        self.outstanding_bytes -= moving;
        self.committed_bytes = self.committed_bytes.saturating_add(moving);
        if let Some(ledger) = self.ledger.upgrade() {
            ledger.apply_commit(self.id, moving);
        }
        self.committed_bytes
    }

    pub fn grow(&mut self, bytes: usize) -> Result<usize, ResourceDenied> {
        if bytes == 0 {
            return Ok(self.held_bytes());
        }
        let Some(ledger) = self.ledger.upgrade() else {
            return Err(ResourceDenied {
                subsystem: self.subsystem,
                owner: self.owner,
                requested_bytes: bytes,
                available_bytes: 0,
                ceiling_bytes: 0,
                scope: DenyScope::DetachedLedger,
            });
        };
        let held = ledger.apply_grow(self.id, bytes)?;
        self.committed_bytes = held.saturating_sub(self.outstanding_bytes);
        Ok(held)
    }

    pub fn shrink(&mut self, bytes: usize) -> usize {
        let from_outstanding = bytes.min(self.outstanding_bytes);
        self.outstanding_bytes -= from_outstanding;
        let from_committed = (bytes - from_outstanding).min(self.committed_bytes);
        self.committed_bytes -= from_committed;
        if let Some(ledger) = self.ledger.upgrade() {
            ledger.apply_release(self.id, from_outstanding, from_committed);
        }
        self.held_bytes()
    }

    pub fn release(&mut self) -> usize {
        let freed = self.held_bytes();
        if let Some(ledger) = self.ledger.upgrade() {
            ledger.apply_release(self.id, self.outstanding_bytes, self.committed_bytes);
        }
        self.outstanding_bytes = 0;
        self.committed_bytes = 0;
        freed
    }

    pub fn is_empty(&self) -> bool {
        self.held_bytes() == 0
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if self.outstanding_bytes == 0 {
            return;
        }
        if let Some(ledger) = self.ledger.upgrade() {
            ledger.apply_release(self.id, self.outstanding_bytes, 0);
        }
        self.outstanding_bytes = 0;
    }
}

#[derive(Debug)]
pub struct Lease {
    reservation: Reservation,
}

impl Lease {
    pub fn bytes(&self) -> usize {
        self.reservation.held_bytes()
    }

    pub fn subsystem(&self) -> SubsystemId {
        self.reservation.subsystem()
    }

    pub fn owner(&self) -> OwnerId {
        self.reservation.owner()
    }

    pub fn grow(&mut self, bytes: usize) -> Result<usize, ResourceDenied> {
        self.reservation.grow(bytes)
    }

    pub fn shrink(&mut self, bytes: usize) -> usize {
        self.reservation.shrink(bytes)
    }

    pub fn release(&mut self) -> usize {
        self.reservation.release()
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        self.reservation.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reservation_is_returned_when_dropped_uncommitted() {
        let ledger = ResourceLedger::new(LedgerLimits::default());
        let reserved_seen = {
            let reservation = ledger
                .reserve(SubsystemId::ImageCache, OwnerId::Global, 4096)
                .expect("reserve");
            assert_eq!(reservation.outstanding_bytes(), 4096);
            ledger.totals().reserved_bytes
        };
        assert_eq!(reserved_seen, 4096);
        let totals = ledger.totals();
        assert_eq!(totals.reserved_bytes, 0);
        assert_eq!(totals.committed_bytes, 0);
        assert_eq!(ledger.snapshot().live_reservations, 0);
    }

    #[test]
    fn commit_moves_outstanding_to_committed_and_survives_drop() {
        let ledger = ResourceLedger::new(LedgerLimits::default());
        let mut reservation = ledger
            .reserve(SubsystemId::DomTree, OwnerId::Tab(3), 8_000)
            .unwrap();
        assert_eq!(reservation.commit(), 8_000);
        assert_eq!(reservation.outstanding_bytes(), 0);
        drop(reservation);
        assert_eq!(ledger.totals().committed_bytes, 8_000);
        assert_eq!(ledger.totals().reserved_bytes, 0);
        assert_eq!(
            ledger
                .snapshot()
                .usage_of(SubsystemId::DomTree)
                .unwrap()
                .committed_bytes,
            8_000
        );
    }

    #[test]
    fn subsystem_ceiling_rejects_before_allocating() {
        let ledger = ResourceLedger::new(LedgerLimits::default());
        ledger.register_subsystem(SubsystemId::Gpu, 1_000);
        let held = ledger
            .reserve(SubsystemId::Gpu, OwnerId::Global, 600)
            .expect("first reserve");
        let denied = ledger
            .reserve(SubsystemId::Gpu, OwnerId::Global, 600)
            .expect_err("must deny");
        assert_eq!(denied.scope, DenyScope::SubsystemCeiling);
        assert_eq!(denied.available_bytes, 400);
        assert_eq!(ledger.totals().denials, 1);
        assert_eq!(
            ledger
                .snapshot()
                .usage_of(SubsystemId::Gpu)
                .unwrap()
                .denials,
            1
        );
        assert_eq!(held.committed_bytes(), 0);
    }

    #[test]
    fn global_hard_limit_caps_the_sum_of_subsystems() {
        let ledger = ResourceLedger::new(LedgerLimits::byte_limits(10, 20));
        ledger.register_subsystem(SubsystemId::DomTree, 100);
        ledger.register_subsystem(SubsystemId::Media, 100);
        let mut held = Vec::new();
        for _ in 0..4 {
            match ledger.reserve(SubsystemId::DomTree, OwnerId::Global, 8) {
                Ok(reservation) => held.push(reservation),
                Err(denied) => {
                    assert_eq!(denied.scope, DenyScope::GlobalHardLimit);
                    break;
                }
            }
        }
        assert_eq!(held.len(), 2);
        let totals = ledger.totals();
        assert!(totals.committed_bytes + totals.reserved_bytes <= 20);
    }

    #[test]
    fn unknown_subsystem_is_refused() {
        let ledger = Arc::new(ResourceLedger {
            limits: Mutex::new(LedgerLimits::default()),
            state: Mutex::new(State::default()),
        });
        let denied = ledger
            .reserve(SubsystemId::StorageWrite, OwnerId::Global, 1)
            .expect_err("unregistered");
        assert_eq!(denied.scope, DenyScope::UnknownSubsystem);
    }

    #[test]
    fn releasing_a_reservation_returns_every_byte() {
        let ledger = ResourceLedger::new(LedgerLimits::default());
        let mut reservation = ledger
            .reserve(SubsystemId::RuntimeHeap, OwnerId::Process(9), 5_000)
            .unwrap();
        reservation.commit();
        assert_eq!(ledger.totals().committed_bytes, 5_000);
        assert_eq!(reservation.release(), 5_000);
        assert_eq!(ledger.totals().committed_bytes, 0);
        assert_eq!(ledger.snapshot().live_reservations, 0);
    }

    #[test]
    fn shrink_returns_unused_quota() {
        let ledger = ResourceLedger::new(LedgerLimits::default());
        let mut reservation = ledger
            .reserve(SubsystemId::ResourceCache, OwnerId::Global, 10_000)
            .unwrap();
        reservation.commit();
        reservation.shrink(4_000);
        assert_eq!(reservation.committed_bytes(), 6_000);
        assert_eq!(ledger.totals().committed_bytes, 6_000);
    }

    #[test]
    fn release_owner_frees_tab_scoped_quota() {
        let ledger = ResourceLedger::new(LedgerLimits::default());
        let mut first = ledger
            .reserve(SubsystemId::DomTree, OwnerId::Tab(7), 3_000)
            .unwrap();
        let mut second = ledger
            .reserve(SubsystemId::LayoutTree, OwnerId::Tab(7), 2_000)
            .unwrap();
        first.commit();
        second.commit();
        assert_eq!(ledger.owner_bytes(OwnerId::Tab(7)), 5_000);
        let freed = ledger.release_owner(OwnerId::Tab(7));
        assert_eq!(freed, 5_000);
        assert_eq!(ledger.totals().committed_bytes, 0);
        assert!(ledger.reservations_for_owner(OwnerId::Tab(7)).is_empty());
    }

    #[test]
    fn grow_extends_the_same_reservation_in_place() {
        let ledger = ResourceLedger::new(LedgerLimits::default());
        let mut reservation = ledger
            .reserve(SubsystemId::DomTree, OwnerId::Tab(1), 1_000)
            .unwrap();
        reservation.commit();
        assert_eq!(reservation.grow(500).unwrap(), 1_500);
        assert_eq!(ledger.totals().committed_bytes, 1_500);
        assert_eq!(ledger.reservations_for_owner(OwnerId::Tab(1)).len(), 1);
        assert_eq!(ledger.snapshot().live_reservations, 1);
    }

    #[test]
    fn grow_refuses_beyond_the_hard_limit_without_double_counting() {
        let ledger = ResourceLedger::new(LedgerLimits::byte_limits(10, 20));
        ledger.register_subsystem(SubsystemId::DomTree, 100);
        let mut reservation = ledger
            .reserve(SubsystemId::DomTree, OwnerId::Global, 15)
            .unwrap();
        reservation.commit();
        assert_eq!(
            reservation.grow(10).unwrap_err().scope,
            DenyScope::GlobalHardLimit
        );
        assert_eq!(ledger.totals().committed_bytes, 15);
        assert_eq!(reservation.committed_bytes(), 15);
    }

    #[test]
    fn lease_releases_itself_on_drop() {
        let ledger = ResourceLedger::new(LedgerLimits::default());
        {
            let lease = ledger
                .lease(SubsystemId::Media, OwnerId::Tab(4), 2_048)
                .unwrap();
            assert_eq!(lease.bytes(), 2_048);
            assert_eq!(ledger.totals().committed_bytes, 2_048);
        }
        assert_eq!(ledger.totals().committed_bytes, 0);
        assert_eq!(ledger.totals().reserved_bytes, 0);
    }

    #[test]
    fn real_memory_aggregates_main_and_children() {
        let ledger = ResourceLedger::new(LedgerLimits::default());
        ledger.note_main_working_set(10_000);
        ledger.note_child_working_set(101, 5_000);
        ledger.note_child_working_set(102, 2_500);
        assert_eq!(ledger.real_working_set_bytes(), 17_500);
        ledger.forget_child(102);
        assert_eq!(ledger.real_working_set_bytes(), 15_000);
        ledger.note_child_working_set(101, 0);
        assert_eq!(ledger.real_working_set_bytes(), 10_000);
    }

    #[test]
    fn aggregate_uses_the_larger_of_committed_and_real() {
        let ledger = ResourceLedger::new(LedgerLimits::default());
        let mut reservation = ledger
            .reserve(SubsystemId::DomTree, OwnerId::Global, 9_000)
            .unwrap();
        reservation.commit();
        ledger.note_main_working_set(4_000);
        assert_eq!(ledger.aggregate_bytes(), 9_000);
        ledger.note_main_working_set(40_000);
        assert_eq!(ledger.aggregate_bytes(), 40_000);
    }

    #[test]
    fn utilisation_percent_tracks_the_ceiling() {
        let ledger = ResourceLedger::new(LedgerLimits::default());
        ledger.register_subsystem(SubsystemId::ImageCache, 100);
        let _lease = ledger
            .lease(SubsystemId::ImageCache, OwnerId::Global, 70)
            .unwrap();
        let snapshot = ledger.snapshot();
        let usage = snapshot.usage_of(SubsystemId::ImageCache).unwrap();
        assert_eq!(usage.utilisation_percent(), 70);
        assert_eq!(
            snapshot.most_saturated_subsystem(),
            Some(SubsystemId::ImageCache)
        );
    }

    #[test]
    fn zero_byte_reservation_is_inert() {
        let ledger = ResourceLedger::new(LedgerLimits::default());
        let reservation = ledger
            .reserve(SubsystemId::DomTree, OwnerId::Global, 0)
            .unwrap();
        assert!(reservation.is_empty());
        assert!(reservation.is_detached());
        assert_eq!(ledger.snapshot().live_reservations, 0);
        assert_eq!(ledger.totals().committed_bytes, 0);
    }

    #[test]
    fn one_thousand_reserve_release_cycles_leak_nothing() {
        let ledger = ResourceLedger::new(LedgerLimits::default());
        for index in 0..1_000 {
            let owner = OwnerId::Tab(index % 20);
            let mut reservation = ledger
                .reserve(SubsystemId::DomTree, owner, 1_024)
                .expect("reserve");
            reservation.commit();
            assert_eq!(reservation.release(), 1_024);
        }
        let totals = ledger.totals();
        assert_eq!(totals.committed_bytes, 0);
        assert_eq!(totals.reserved_bytes, 0);
        assert_eq!(ledger.snapshot().live_reservations, 0);
        assert_eq!(ledger.grant_count(), 1_000);
    }

    #[test]
    fn committed_quota_survives_ledger_drop_without_panicking() {
        let ledger = ResourceLedger::new(LedgerLimits::default());
        let mut reservation = ledger
            .reserve(SubsystemId::DomTree, OwnerId::Global, 1_000)
            .unwrap();
        reservation.commit();
        let mut detached = reservation;
        drop(ledger);
        assert!(detached.is_detached());
        detached.shrink(10);
        assert_eq!(detached.committed_bytes(), 990);
    }

    #[test]
    fn default_ceilings_are_registered_on_construction() {
        let ledger = ResourceLedger::new(LedgerLimits::default());
        for subsystem in SubsystemId::all() {
            assert!(ledger.is_registered(subsystem));
            assert_eq!(
                ledger.ceiling_bytes(subsystem),
                subsystem.default_ceiling_bytes()
            );
        }
        assert!(ledger.aggregate_bytes() < ledger.limits().hard_bytes);
    }

    #[test]
    fn reconcile_converges_the_owner_quota_without_duplicates() {
        let ledger = ResourceLedger::new(LedgerLimits::default());
        assert_eq!(
            ledger.reconcile(SubsystemId::DomTree, OwnerId::Tab(5), 4_000),
            4_000
        );
        assert_eq!(
            ledger.reconcile(SubsystemId::DomTree, OwnerId::Tab(5), 4_000),
            4_000
        );
        assert_eq!(
            ledger.reconcile(SubsystemId::DomTree, OwnerId::Tab(5), 9_000),
            9_000
        );
        assert_eq!(
            ledger.reconcile(SubsystemId::DomTree, OwnerId::Tab(5), 2_000),
            2_000
        );
        assert_eq!(ledger.totals().committed_bytes, 2_000);
        assert_eq!(ledger.reservations_for_owner(OwnerId::Tab(5)).len(), 1);
        assert_eq!(ledger.snapshot().live_reservations, 1);
        ledger.forget_reconciled(SubsystemId::DomTree, OwnerId::Tab(5));
        assert_eq!(ledger.totals().committed_bytes, 0);
        assert_eq!(ledger.snapshot().live_reservations, 0);
    }

    #[test]
    fn release_owner_also_drops_reconciled_records() {
        let ledger = ResourceLedger::new(LedgerLimits::default());
        ledger.reconcile(SubsystemId::LayoutTree, OwnerId::Tab(8), 3_000);
        ledger.reconcile(SubsystemId::RuntimeHeap, OwnerId::Tab(8), 1_500);
        assert_eq!(ledger.owner_bytes(OwnerId::Tab(8)), 4_500);
        assert_eq!(ledger.release_owner(OwnerId::Tab(8)), 4_500);
        assert_eq!(ledger.totals().committed_bytes, 0);
        ledger.reconcile(SubsystemId::LayoutTree, OwnerId::Tab(8), 500);
        assert_eq!(ledger.totals().committed_bytes, 500);
    }

    #[test]
    fn denial_counters_accumulate_across_subsystems() {
        let ledger = ResourceLedger::new(LedgerLimits::byte_limits(8_000, 8_000));
        ledger.register_subsystem(SubsystemId::Gpu, 4_000);
        let _held = ledger
            .reserve(SubsystemId::Gpu, OwnerId::Global, 1_000)
            .expect("within ceiling");
        assert!(ledger
            .reserve(SubsystemId::Gpu, OwnerId::Global, 9_000)
            .is_err());
        assert!(ledger
            .reserve(SubsystemId::Media, OwnerId::Global, 9_000)
            .is_err());
        let totals = ledger.totals();
        assert_eq!(totals.denials, 2);
        assert_eq!(totals.denial_bytes, 18_000);
    }

    #[test]
    fn reconcile_to_zero_then_grow_reaccounts_the_owner() {
        let ledger = ResourceLedger::new(LedgerLimits::default());
        assert_eq!(
            ledger.reconcile(SubsystemId::RuntimeHeap, OwnerId::Tab(1), 6_000),
            6_000
        );
        assert_eq!(
            ledger.reconcile(SubsystemId::RuntimeHeap, OwnerId::Tab(1), 0),
            0
        );
        assert_eq!(ledger.totals().committed_bytes, 0);
        assert_eq!(
            ledger.reconcile(SubsystemId::RuntimeHeap, OwnerId::Tab(1), 4_000),
            4_000
        );
        assert_eq!(ledger.totals().committed_bytes, 4_000);
        assert_eq!(ledger.owner_bytes(OwnerId::Tab(1)), 4_000);
        let usage = ledger
            .snapshot()
            .usage_of(SubsystemId::RuntimeHeap)
            .expect("subsystem tracked");
        assert_eq!(usage.committed_bytes, 4_000);
    }

    #[test]
    fn sleeping_tab_then_wake_keeps_accounting_across_pressure_cycles() {
        let ledger = ResourceLedger::new(LedgerLimits::default());
        for cycle in 0..3 {
            let dom = ledger.reconcile(SubsystemId::DomTree, OwnerId::Tab(5), 10_000);
            let heap = ledger.reconcile(SubsystemId::RuntimeHeap, OwnerId::Tab(5), 5_000);
            assert_eq!(dom + heap, 15_000, "cycle {cycle}");
            assert_eq!(ledger.owner_bytes(OwnerId::Tab(5)), 15_000);
            assert_eq!(
                ledger.reconcile(SubsystemId::DomTree, OwnerId::Tab(5), 0),
                0
            );
            assert_eq!(
                ledger.reconcile(SubsystemId::RuntimeHeap, OwnerId::Tab(5), 0),
                0
            );
            assert_eq!(ledger.owner_bytes(OwnerId::Tab(5)), 0);
        }
        assert_eq!(
            ledger.reconcile(SubsystemId::DomTree, OwnerId::Tab(5), 7_000),
            7_000
        );
        assert_eq!(ledger.totals().committed_bytes, 7_000);
    }

    #[test]
    fn release_owner_drops_stale_reconciled_entries() {
        let ledger = ResourceLedger::new(LedgerLimits::default());
        assert_eq!(
            ledger.reconcile(SubsystemId::LayoutTree, OwnerId::Tab(8), 3_000),
            3_000
        );
        ledger.forget_reconciled(SubsystemId::LayoutTree, OwnerId::Tab(8));
        assert_eq!(
            ledger.reconcile(SubsystemId::LayoutTree, OwnerId::Tab(8), 2_000),
            2_000
        );
        assert_eq!(ledger.totals().committed_bytes, 2_000);
        ledger.release_owner(OwnerId::Tab(8));
        assert_eq!(
            ledger.reconcile(SubsystemId::LayoutTree, OwnerId::Tab(8), 1_000),
            1_000
        );
        assert_eq!(ledger.totals().committed_bytes, 1_000);
    }

    #[test]
    fn first_reservation_is_not_reported_as_detached() {
        let ledger = ResourceLedger::new(LedgerLimits::default());
        let mut reservation = ledger
            .reserve(SubsystemId::ImageCache, OwnerId::Global, 1_000)
            .expect("reserve");
        assert!(!reservation.is_detached());
        reservation.commit();
        assert_eq!(ledger.totals().committed_bytes, 1_000);
    }
}
