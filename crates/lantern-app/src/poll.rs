use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use lantern_domain::{
    DataBits, LinkSettings, ModbusFunction, ModbusTable, ParameterId, Parity, RawRegisters,
    RegisterAddress, RegisterBlock, RegisterCount, RequestId, SessionId, SlaveId, StopBits,
};
use lantern_profile::{ValidatedDeviceProfile, ValidatedParameter};
use thiserror::Error;
use tokio::{
    sync::{mpsc, watch},
    task::JoinHandle,
};

use crate::{
    BusError, BusRequestContext, MonotonicClock, ReadBusPort, ReadBusRequest, RequestClass,
};

const ONE_MILLION_PPM: u32 = 1_000_000;
const DEFAULT_POLL_BUDGET_PPM: u32 = 700_000;
const MODBUS_READ_REQUEST_BYTES: u64 = 8;
const MODBUS_READ_RESPONSE_FIXED_BYTES: u64 = 5;
const MAX_SUBSCRIBER_ID_BYTES: usize = 128;

/// Application-level cadence requested by a telemetry consumer.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum FrequencyClass {
    Fast,
    Normal,
    Slow,
    OnDemand,
}

impl FrequencyClass {
    const fn next_slower(self) -> Option<Self> {
        match self {
            Self::Fast => Some(Self::Normal),
            Self::Normal => Some(Self::Slow),
            Self::Slow | Self::OnDemand => None,
        }
    }
}

/// Closed application reason used to map subscriptions to bus queue classes.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SubscriptionReason {
    DriveState,
    Fault,
    Dashboard,
    Scope,
    Csv,
    ParameterBrowser,
    Backup,
    Diagnostics,
}

impl SubscriptionReason {
    const fn periodic_class(self) -> RequestClass {
        match self {
            Self::DriveState | Self::Fault => RequestClass::TelemetryCritical,
            Self::Dashboard | Self::Scope | Self::Csv | Self::ParameterBrowser => {
                RequestClass::Telemetry
            }
            Self::Backup | Self::Diagnostics => RequestClass::Background,
        }
    }

    const fn budget_rank(self) -> u8 {
        match self {
            Self::Diagnostics => 0,
            Self::Backup => 1,
            Self::Csv => 2,
            Self::Scope => 3,
            Self::Dashboard | Self::ParameterBrowser => 4,
            Self::Fault => 5,
            Self::DriveState => 6,
        }
    }
}

/// Stable identifier of one subscription producer.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SubscriberId(String);

impl SubscriberId {
    pub fn parse(value: impl Into<String>) -> Result<Self, PollPlanError> {
        let value = value.into();
        if value.is_empty() {
            return Err(PollPlanError::InvalidSubscriberId("identifier is empty"));
        }
        if value.len() > MAX_SUBSCRIBER_ID_BYTES {
            return Err(PollPlanError::InvalidSubscriberId("identifier is too long"));
        }
        if !value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | ':')
        }) {
            return Err(PollPlanError::InvalidSubscriberId(
                "identifier contains a non-portable character",
            ));
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One consumer request for a parameter value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadSubscription {
    parameter_id: ParameterId,
    frequency: FrequencyClass,
    subscriber_id: SubscriberId,
    reason: SubscriptionReason,
    history_required: bool,
    maximum_age: Duration,
}

impl ReadSubscription {
    pub fn new(
        parameter_id: ParameterId,
        frequency: FrequencyClass,
        subscriber_id: SubscriberId,
        reason: SubscriptionReason,
        history_required: bool,
        maximum_age: Duration,
    ) -> Result<Self, PollPlanError> {
        if maximum_age.is_zero() {
            return Err(PollPlanError::ZeroMaximumAge);
        }
        Ok(Self {
            parameter_id,
            frequency,
            subscriber_id,
            reason,
            history_required,
            maximum_age,
        })
    }

    #[must_use]
    pub fn parameter_id(&self) -> &ParameterId {
        &self.parameter_id
    }

    #[must_use]
    pub const fn frequency(&self) -> FrequencyClass {
        self.frequency
    }

    #[must_use]
    pub const fn reason(&self) -> SubscriptionReason {
        self.reason
    }

    #[must_use]
    pub fn subscriber_id(&self) -> &SubscriberId {
        &self.subscriber_id
    }

    #[must_use]
    pub const fn history_required(&self) -> bool {
        self.history_required
    }

    #[must_use]
    pub const fn maximum_age(&self) -> Duration {
        self.maximum_age
    }
}

/// Product cadence policy. These durations are application-owned, not UI timers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PollCadences {
    fast: Duration,
    normal: Duration,
    slow: Duration,
}

impl PollCadences {
    pub fn new(fast: Duration, normal: Duration, slow: Duration) -> Result<Self, PollPlanError> {
        if fast.is_zero() || normal.is_zero() || slow.is_zero() {
            return Err(PollPlanError::ZeroCadence);
        }
        if !(fast <= normal && normal <= slow) {
            return Err(PollPlanError::InvalidCadenceOrder);
        }
        Ok(Self { fast, normal, slow })
    }

    #[must_use]
    pub const fn period(self, frequency: FrequencyClass) -> Option<Duration> {
        match frequency {
            FrequencyClass::Fast => Some(self.fast),
            FrequencyClass::Normal => Some(self.normal),
            FrequencyClass::Slow => Some(self.slow),
            FrequencyClass::OnDemand => None,
        }
    }
}

impl Default for PollCadences {
    fn default() -> Self {
        Self {
            fast: Duration::from_millis(100),
            normal: Duration::from_secs(1),
            slow: Duration::from_secs(10),
        }
    }
}

/// Immutable planner inputs that affect RTU utilization.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PollPlannerConfig {
    cadences: PollCadences,
    link: LinkSettings,
    measured_response_time: Duration,
    device_margin: Duration,
    budget_ppm: u32,
}

impl PollPlannerConfig {
    pub fn new(
        cadences: PollCadences,
        link: LinkSettings,
        measured_response_time: Duration,
        device_margin: Duration,
        budget_ppm: u32,
    ) -> Result<Self, PollPlanError> {
        if budget_ppm == 0 || budget_ppm > DEFAULT_POLL_BUDGET_PPM {
            return Err(PollPlanError::InvalidBudgetPpm(budget_ppm));
        }
        Ok(Self {
            cadences,
            link,
            measured_response_time,
            device_margin,
            budget_ppm,
        })
    }

    #[must_use]
    pub fn for_profile(profile: &ValidatedDeviceProfile) -> Self {
        Self {
            cadences: PollCadences::default(),
            link: profile.protocol().default_link(),
            measured_response_time: Duration::ZERO,
            device_margin: Duration::ZERO,
            budget_ppm: DEFAULT_POLL_BUDGET_PPM,
        }
    }

    #[must_use]
    pub const fn cadences(self) -> PollCadences {
        self.cadences
    }

    #[must_use]
    pub const fn link(self) -> LinkSettings {
        self.link
    }

    #[must_use]
    pub const fn budget_ppm(self) -> u32 {
        self.budget_ppm
    }
}

/// One parameter slice contained in a shared Modbus block.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PollParameterSlice {
    parameter_id: ParameterId,
    register_offset: u16,
    register_count: RegisterCount,
    subscribers: Box<[SubscriberId]>,
    reasons: Box<[SubscriptionReason]>,
    history_required: bool,
    maximum_age: Duration,
}

impl PollParameterSlice {
    #[must_use]
    pub fn parameter_id(&self) -> &ParameterId {
        &self.parameter_id
    }

    #[must_use]
    pub const fn register_offset(&self) -> u16 {
        self.register_offset
    }

    #[must_use]
    pub const fn register_count(&self) -> RegisterCount {
        self.register_count
    }

    #[must_use]
    pub fn subscribers(&self) -> &[SubscriberId] {
        &self.subscribers
    }

    #[must_use]
    pub fn reasons(&self) -> &[SubscriptionReason] {
        &self.reasons
    }

    #[must_use]
    pub const fn history_required(&self) -> bool {
        self.history_required
    }

    #[must_use]
    pub const fn maximum_age(&self) -> Duration {
        self.maximum_age
    }
}

/// Justification retained for every planned block.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PollBlockRationale {
    parameters: Box<[ParameterId]>,
    subscribers: Box<[SubscriberId]>,
    reasons: Box<[SubscriptionReason]>,
}

impl PollBlockRationale {
    #[must_use]
    pub fn parameters(&self) -> &[ParameterId] {
        &self.parameters
    }

    #[must_use]
    pub fn subscribers(&self) -> &[SubscriberId] {
        &self.subscribers
    }

    #[must_use]
    pub fn reasons(&self) -> &[SubscriptionReason] {
        &self.reasons
    }
}

/// One immutable periodic RTU block.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PollBlock {
    index: u32,
    slave: SlaveId,
    function: ModbusFunction,
    block: RegisterBlock,
    request_class: RequestClass,
    frequency: FrequencyClass,
    period: Duration,
    next_due: Instant,
    maximum_age: Duration,
    estimated_cost: Duration,
    utilization_ppm: u32,
    parameters: Box<[PollParameterSlice]>,
    rationale: PollBlockRationale,
}

impl PollBlock {
    #[must_use]
    pub const fn index(&self) -> u32 {
        self.index
    }

    #[must_use]
    pub const fn slave(&self) -> SlaveId {
        self.slave
    }

    #[must_use]
    pub const fn function(&self) -> ModbusFunction {
        self.function
    }

    #[must_use]
    pub const fn block(&self) -> RegisterBlock {
        self.block
    }

    #[must_use]
    pub const fn request_class(&self) -> RequestClass {
        self.request_class
    }

    #[must_use]
    pub const fn frequency(&self) -> FrequencyClass {
        self.frequency
    }

    #[must_use]
    pub const fn period(&self) -> Duration {
        self.period
    }

    #[must_use]
    pub const fn next_due(&self) -> Instant {
        self.next_due
    }

    #[must_use]
    pub const fn maximum_age(&self) -> Duration {
        self.maximum_age
    }

    #[must_use]
    pub const fn estimated_cost(&self) -> Duration {
        self.estimated_cost
    }

    #[must_use]
    pub const fn utilization_ppm(&self) -> u32 {
        self.utilization_ppm
    }

    #[must_use]
    pub fn parameters(&self) -> &[PollParameterSlice] {
        &self.parameters
    }

    #[must_use]
    pub const fn rationale(&self) -> &PollBlockRationale {
        &self.rationale
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PollDegradation {
    parameter_id: ParameterId,
    from: FrequencyClass,
    to: FrequencyClass,
}

impl PollDegradation {
    #[must_use]
    pub fn parameter_id(&self) -> &ParameterId {
        &self.parameter_id
    }

    #[must_use]
    pub const fn from(&self) -> FrequencyClass {
        self.from
    }

    #[must_use]
    pub const fn to(&self) -> FrequencyClass {
        self.to
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PollRejectionReason {
    UnknownParameter,
    OnDemandOnly,
    MaximumAgeBelowFastCadence,
    BudgetExceeded,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PollRejection {
    parameter_id: ParameterId,
    subscribers: Box<[SubscriberId]>,
    reason: PollRejectionReason,
}

impl PollRejection {
    #[must_use]
    pub fn parameter_id(&self) -> &ParameterId {
        &self.parameter_id
    }

    #[must_use]
    pub fn subscribers(&self) -> &[SubscriberId] {
        &self.subscribers
    }

    #[must_use]
    pub const fn reason(&self) -> PollRejectionReason {
        self.reason
    }
}

/// One immutable, versioned polling plan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PollPlan {
    version: u64,
    created_at: Instant,
    slave: SlaveId,
    blocks: Box<[PollBlock]>,
    degradations: Box<[PollDegradation]>,
    rejections: Box<[PollRejection]>,
    utilization_ppm: u32,
    budget_ppm: u32,
}

impl PollPlan {
    #[must_use]
    pub const fn version(&self) -> u64 {
        self.version
    }

    #[must_use]
    pub const fn created_at(&self) -> Instant {
        self.created_at
    }

    #[must_use]
    pub const fn slave(&self) -> SlaveId {
        self.slave
    }

    #[must_use]
    pub fn blocks(&self) -> &[PollBlock] {
        &self.blocks
    }

    #[must_use]
    pub fn degradations(&self) -> &[PollDegradation] {
        &self.degradations
    }

    #[must_use]
    pub fn rejections(&self) -> &[PollRejection] {
        &self.rejections
    }

    #[must_use]
    pub const fn utilization_ppm(&self) -> u32 {
        self.utilization_ppm
    }

    #[must_use]
    pub const fn budget_ppm(&self) -> u32 {
        self.budget_ppm
    }
}

#[derive(Debug, Error, Clone, Eq, PartialEq)]
pub enum PollPlanError {
    #[error("subscriber identifier is invalid: {0}")]
    InvalidSubscriberId(&'static str),
    #[error("maximum sample age must be non-zero")]
    ZeroMaximumAge,
    #[error("poll cadence must be non-zero")]
    ZeroCadence,
    #[error("poll cadences must satisfy fast <= normal <= slow")]
    InvalidCadenceOrder,
    #[error("cyclic bus budget {0} ppm is outside 1..=700000")]
    InvalidBudgetPpm(u32),
    #[error("poll plan version overflow")]
    VersionOverflow,
    #[error("time arithmetic overflow")]
    TimeOverflow,
    #[error("invalid grouped Modbus block")]
    InvalidGroupedBlock,
}

#[derive(Clone, Debug)]
struct Demand {
    parameter_id: ParameterId,
    block: RegisterBlock,
    frequency: FrequencyClass,
    request_class: RequestClass,
    maximum_age: Duration,
    subscribers: BTreeSet<SubscriberId>,
    reasons: BTreeSet<SubscriptionReason>,
    history_required: bool,
    do_not_bridge: bool,
    maximum_bridge_gap: u16,
    budget_rank: u8,
}

impl Demand {
    fn from_parameter(
        parameter: &ValidatedParameter,
        subscriptions: &[&ReadSubscription],
        cadences: PollCadences,
    ) -> Result<Option<Self>, PollRejectionReason> {
        let mut cyclic = subscriptions
            .iter()
            .copied()
            .filter(|subscription| subscription.frequency != FrequencyClass::OnDemand)
            .collect::<Vec<_>>();
        if cyclic.is_empty() {
            return Ok(None);
        }
        cyclic.sort_by(|left, right| {
            left.frequency
                .cmp(&right.frequency)
                .then_with(|| left.reason.cmp(&right.reason))
                .then_with(|| left.subscriber_id.cmp(&right.subscriber_id))
        });
        let requested = cyclic[0].frequency;
        let maximum_age = cyclic
            .iter()
            .map(|subscription| subscription.maximum_age)
            .min()
            .expect("cyclic subscriptions are non-empty");
        let frequency = frequency_for_age(requested, maximum_age, cadences)
            .ok_or(PollRejectionReason::MaximumAgeBelowFastCadence)?;
        let request_class = cyclic
            .iter()
            .map(|subscription| subscription.reason.periodic_class())
            .max_by_key(|class| request_class_rank(*class))
            .expect("cyclic subscriptions are non-empty");
        let subscribers = subscriptions
            .iter()
            .map(|subscription| subscription.subscriber_id.clone())
            .collect();
        let reasons = cyclic
            .iter()
            .map(|subscription| subscription.reason)
            .collect::<BTreeSet<_>>();
        let budget_rank = cyclic
            .iter()
            .map(|subscription| subscription.reason.budget_rank())
            .max()
            .unwrap_or(0);
        Ok(Some(Self {
            parameter_id: parameter.id().clone(),
            block: parameter.block(),
            frequency,
            request_class,
            maximum_age,
            subscribers,
            reasons,
            history_required: subscriptions
                .iter()
                .any(|subscription| subscription.history_required),
            do_not_bridge: parameter.do_not_bridge(),
            maximum_bridge_gap: parameter.maximum_bridge_gap(),
            budget_rank,
        }))
    }
}

/// Pure planner for the application-owned cyclic read plan.
#[derive(Debug, Default)]
pub struct PollPlanner {
    next_version: Mutex<u64>,
}

impl PollPlanner {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn build(
        &self,
        profile: &ValidatedDeviceProfile,
        subscriptions: impl IntoIterator<Item = ReadSubscription>,
        config: PollPlannerConfig,
        now: Instant,
    ) -> Result<PollPlan, PollPlanError> {
        let version = {
            let mut next = self
                .next_version
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *next = next.checked_add(1).ok_or(PollPlanError::VersionOverflow)?;
            *next
        };
        build_plan(version, profile, subscriptions, config, now)
    }
}

fn build_plan(
    version: u64,
    profile: &ValidatedDeviceProfile,
    subscriptions: impl IntoIterator<Item = ReadSubscription>,
    config: PollPlannerConfig,
    now: Instant,
) -> Result<PollPlan, PollPlanError> {
    let mut grouped = BTreeMap::<ParameterId, Vec<ReadSubscription>>::new();
    for subscription in subscriptions {
        grouped
            .entry(subscription.parameter_id.clone())
            .or_default()
            .push(subscription);
    }

    let mut active = BTreeMap::<ParameterId, Demand>::new();
    let mut rejections = Vec::new();
    for (parameter_id, mut parameter_subscriptions) in grouped {
        parameter_subscriptions.sort_by(|left, right| {
            left.frequency
                .cmp(&right.frequency)
                .then_with(|| left.reason.cmp(&right.reason))
                .then_with(|| left.subscriber_id.cmp(&right.subscriber_id))
        });
        let subscribers = parameter_subscriptions
            .iter()
            .map(|subscription| subscription.subscriber_id.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let Some(parameter) = profile.parameter(&parameter_id) else {
            rejections.push(PollRejection {
                parameter_id,
                subscribers: subscribers.into_boxed_slice(),
                reason: PollRejectionReason::UnknownParameter,
            });
            continue;
        };
        let refs = parameter_subscriptions.iter().collect::<Vec<_>>();
        match Demand::from_parameter(parameter, &refs, config.cadences) {
            Ok(Some(demand)) => {
                active.insert(parameter_id, demand);
            }
            Ok(None) => rejections.push(PollRejection {
                parameter_id,
                subscribers: subscribers.into_boxed_slice(),
                reason: PollRejectionReason::OnDemandOnly,
            }),
            Err(reason) => rejections.push(PollRejection {
                parameter_id,
                subscribers: subscribers.into_boxed_slice(),
                reason,
            }),
        }
    }

    let mut degradations = Vec::new();
    loop {
        let draft = build_blocks(profile, active.values(), config, now)?;
        if draft.utilization_ppm <= config.budget_ppm || active.is_empty() {
            return Ok(PollPlan {
                version,
                created_at: now,
                slave: config.link.slave_id,
                blocks: finalize_blocks(version, draft.blocks, now)?,
                degradations: degradations.into_boxed_slice(),
                rejections: rejections.into_boxed_slice(),
                utilization_ppm: draft.utilization_ppm,
                budget_ppm: config.budget_ppm,
            });
        }

        let candidate_id = active
            .values()
            .min_by(|left, right| demand_priority_key(left).cmp(&demand_priority_key(right)))
            .map(|demand| demand.parameter_id.clone())
            .expect("active demands are non-empty");
        let demand = active
            .get_mut(&candidate_id)
            .expect("candidate came from active demands");
        if let Some(next) = demand.frequency.next_slower()
            && config
                .cadences
                .period(next)
                .is_some_and(|period| period <= demand.maximum_age)
        {
            let from = demand.frequency;
            demand.frequency = next;
            degradations.push(PollDegradation {
                parameter_id: candidate_id,
                from,
                to: next,
            });
        } else {
            let rejected = active
                .remove(&candidate_id)
                .expect("candidate came from active demands");
            rejections.push(PollRejection {
                parameter_id: rejected.parameter_id,
                subscribers: rejected
                    .subscribers
                    .into_iter()
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
                reason: PollRejectionReason::BudgetExceeded,
            });
        }
    }
}

fn demand_priority_key(demand: &Demand) -> (u8, u8, &str) {
    (
        request_class_rank(demand.request_class),
        demand.budget_rank,
        demand.parameter_id.as_str(),
    )
}

fn request_class_rank(class: RequestClass) -> u8 {
    match class {
        RequestClass::Background => 0,
        RequestClass::Telemetry => 1,
        RequestClass::TelemetryCritical => 2,
        RequestClass::Interactive => 3,
        RequestClass::SafetyOneShot => 4,
    }
}

fn frequency_for_age(
    requested: FrequencyClass,
    maximum_age: Duration,
    cadences: PollCadences,
) -> Option<FrequencyClass> {
    let mut effective = requested;
    while cadences
        .period(effective)
        .is_some_and(|period| period > maximum_age)
    {
        effective = match effective {
            FrequencyClass::Slow => FrequencyClass::Normal,
            FrequencyClass::Normal => FrequencyClass::Fast,
            FrequencyClass::Fast | FrequencyClass::OnDemand => return None,
        };
    }
    Some(effective)
}

#[derive(Debug)]
struct DraftPlan {
    blocks: Vec<DraftBlock>,
    utilization_ppm: u32,
}

#[derive(Clone, Debug)]
struct DraftBlock {
    slave: SlaveId,
    function: ModbusFunction,
    block: RegisterBlock,
    request_class: RequestClass,
    frequency: FrequencyClass,
    period: Duration,
    maximum_age: Duration,
    estimated_cost: Duration,
    utilization_ppm: u32,
    parameters: Vec<Demand>,
}

fn build_blocks<'a>(
    profile: &ValidatedDeviceProfile,
    demands: impl IntoIterator<Item = &'a Demand>,
    config: PollPlannerConfig,
    _now: Instant,
) -> Result<DraftPlan, PollPlanError> {
    let mut sorted = demands.into_iter().cloned().collect::<Vec<_>>();
    sorted.sort_by(|left, right| {
        group_key(left)
            .cmp(&group_key(right))
            .then_with(|| left.block.start().cmp(&right.block.start()))
            .then_with(|| left.parameter_id.cmp(&right.parameter_id))
    });

    let mut groups = Vec::<Vec<Demand>>::new();
    for demand in sorted {
        if let Some(last) = groups.last_mut()
            && can_append(last, &demand)
        {
            last.push(demand);
            continue;
        }
        groups.push(vec![demand]);
    }

    let mut blocks = Vec::with_capacity(groups.len());
    let mut total_ppm = 0_u32;
    for parameters in groups {
        let first = parameters.first().expect("groups are non-empty");
        let start = parameters
            .iter()
            .map(|demand| demand.block.start().get())
            .min()
            .expect("groups are non-empty");
        let end = parameters
            .iter()
            .map(|demand| demand.block.end().get())
            .max()
            .expect("groups are non-empty");
        let count = end
            .checked_sub(start)
            .and_then(|span| span.checked_add(1))
            .ok_or(PollPlanError::InvalidGroupedBlock)?;
        let count = RegisterCount::new(count).map_err(|_| PollPlanError::InvalidGroupedBlock)?;
        let function = read_function(first.block.table());
        let block = RegisterBlock::new(
            first.block.table(),
            RegisterAddress::new(start),
            count,
            function,
        )
        .map_err(|_| PollPlanError::InvalidGroupedBlock)?;
        let period = config
            .cadences
            .period(first.frequency)
            .expect("active demand is periodic");
        let cost = estimate_read_transaction_cost(
            config.link,
            block,
            profile.protocol().minimum_inter_frame_delay(),
            config.measured_response_time,
            config.device_margin,
        );
        let ppm = utilization_ppm(cost, period);
        total_ppm = total_ppm.saturating_add(ppm);
        blocks.push(DraftBlock {
            slave: config.link.slave_id,
            function,
            block,
            request_class: first.request_class,
            frequency: first.frequency,
            period,
            maximum_age: parameters
                .iter()
                .map(|demand| demand.maximum_age)
                .min()
                .expect("groups are non-empty"),
            estimated_cost: cost,
            utilization_ppm: ppm,
            parameters,
        });
    }
    Ok(DraftPlan {
        blocks,
        utilization_ppm: total_ppm,
    })
}

fn group_key(demand: &Demand) -> (u8, FrequencyClass, u8) {
    (
        request_class_rank(demand.request_class),
        demand.frequency,
        table_rank(demand.block.table()),
    )
}

fn table_rank(table: ModbusTable) -> u8 {
    match table {
        ModbusTable::InputRegisters => 0,
        ModbusTable::HoldingRegisters => 1,
    }
}

fn can_append(group: &[Demand], next: &Demand) -> bool {
    let first = group.first().expect("group is non-empty");
    if group_key(first) != group_key(next) {
        return false;
    }
    let start = group
        .iter()
        .map(|demand| demand.block.start().get())
        .min()
        .expect("group is non-empty");
    let end = group
        .iter()
        .map(|demand| demand.block.end().get())
        .max()
        .expect("group is non-empty");
    let combined_end = end.max(next.block.end().get());
    if u32::from(combined_end) - u32::from(start) + 1 > 125 {
        return false;
    }
    if next.block.start().get() <= end.saturating_add(1) {
        return true;
    }
    let gap = next.block.start().get() - end - 1;
    let group_do_not_bridge = group.iter().any(|demand| demand.do_not_bridge);
    let group_gap = group
        .iter()
        .map(|demand| demand.maximum_bridge_gap)
        .min()
        .unwrap_or(0);
    !group_do_not_bridge && !next.do_not_bridge && gap <= group_gap.min(next.maximum_bridge_gap)
}

fn finalize_blocks(
    _version: u64,
    mut drafts: Vec<DraftBlock>,
    now: Instant,
) -> Result<Box<[PollBlock]>, PollPlanError> {
    drafts.sort_by(|left, right| {
        left.period
            .cmp(&right.period)
            .then_with(|| {
                request_class_rank(right.request_class).cmp(&request_class_rank(left.request_class))
            })
            .then_with(|| table_rank(left.block.table()).cmp(&table_rank(right.block.table())))
            .then_with(|| left.block.start().cmp(&right.block.start()))
    });
    let mut total_per_period = BTreeMap::<Duration, usize>::new();
    for draft in &drafts {
        *total_per_period.entry(draft.period).or_default() += 1;
    }
    let mut seen_per_period = BTreeMap::<Duration, usize>::new();
    let mut blocks = Vec::with_capacity(drafts.len());
    for (index, draft) in drafts.into_iter().enumerate() {
        let seen = seen_per_period.entry(draft.period).or_default();
        let total = total_per_period[&draft.period];
        let phase = proportional_duration(draft.period, *seen, total);
        *seen += 1;
        let next_due = now.checked_add(phase).ok_or(PollPlanError::TimeOverflow)?;
        let mut subscribers = BTreeSet::new();
        let mut reasons = BTreeSet::new();
        let mut slices = Vec::with_capacity(draft.parameters.len());
        for demand in draft.parameters {
            subscribers.extend(demand.subscribers.iter().cloned());
            reasons.extend(demand.reasons.iter().copied());
            let offset = demand
                .block
                .start()
                .get()
                .checked_sub(draft.block.start().get())
                .ok_or(PollPlanError::InvalidGroupedBlock)?;
            slices.push(PollParameterSlice {
                parameter_id: demand.parameter_id,
                register_offset: offset,
                register_count: demand.block.count(),
                subscribers: demand
                    .subscribers
                    .into_iter()
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
                reasons: demand
                    .reasons
                    .into_iter()
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
                history_required: demand.history_required,
                maximum_age: demand.maximum_age,
            });
        }
        slices.sort_by(|left, right| {
            left.register_offset
                .cmp(&right.register_offset)
                .then_with(|| left.parameter_id.cmp(&right.parameter_id))
        });
        let rationale = PollBlockRationale {
            parameters: slices
                .iter()
                .map(|slice| slice.parameter_id.clone())
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            subscribers: subscribers
                .into_iter()
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            reasons: reasons.into_iter().collect::<Vec<_>>().into_boxed_slice(),
        };
        blocks.push(PollBlock {
            index: u32::try_from(index).unwrap_or(u32::MAX),
            slave: draft.slave,
            function: draft.function,
            block: draft.block,
            request_class: draft.request_class,
            frequency: draft.frequency,
            period: draft.period,
            next_due,
            maximum_age: draft.maximum_age,
            estimated_cost: draft.estimated_cost,
            utilization_ppm: draft.utilization_ppm,
            parameters: slices.into_boxed_slice(),
            rationale,
        });
    }
    Ok(blocks.into_boxed_slice())
}

fn proportional_duration(period: Duration, index: usize, total: usize) -> Duration {
    if index == 0 || total <= 1 {
        return Duration::ZERO;
    }
    let nanos = period.as_nanos().saturating_mul(index as u128) / total as u128;
    Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
}

fn read_function(table: ModbusTable) -> ModbusFunction {
    match table {
        ModbusTable::InputRegisters => ModbusFunction::ReadInputRegisters,
        ModbusTable::HoldingRegisters => ModbusFunction::ReadHoldingRegisters,
    }
}

/// Estimates one RTU read transaction without floating-point arithmetic.
#[must_use]
pub fn estimate_read_transaction_cost(
    link: LinkSettings,
    block: RegisterBlock,
    profile_minimum_inter_frame_delay: Duration,
    measured_response_time: Duration,
    device_margin: Duration,
) -> Duration {
    let response_bytes = MODBUS_READ_RESPONSE_FIXED_BYTES + u64::from(block.count().get()) * 2;
    let wire_bytes = MODBUS_READ_REQUEST_BYTES.saturating_add(response_bytes);
    let bits_per_character = u64::from(serial_bits_per_character(link));
    let wire_micros = wire_bytes
        .saturating_mul(bits_per_character)
        .saturating_mul(1_000_000)
        .div_ceil(u64::from(link.baud_rate.get()));
    let t35 = protocol_t35(link).max(profile_minimum_inter_frame_delay);
    Duration::from_micros(wire_micros)
        .saturating_add(t35)
        .saturating_add(measured_response_time)
        .saturating_add(device_margin)
}

fn serial_bits_per_character(link: LinkSettings) -> u32 {
    let data = match link.data_bits {
        DataBits::Seven => 7,
        DataBits::Eight => 8,
    };
    let parity = u32::from(!matches!(link.parity, Parity::None));
    let stop = match link.stop_bits {
        StopBits::One => 1,
        StopBits::Two => 2,
    };
    1 + data + parity + stop
}

fn protocol_t35(link: LinkSettings) -> Duration {
    if link.baud_rate.get() > 19_200 {
        return Duration::from_micros(1_750);
    }
    let numerator = u64::from(serial_bits_per_character(link)) * 35 * 1_000_000;
    let denominator = u64::from(link.baud_rate.get()) * 10;
    Duration::from_micros(numerator.div_ceil(denominator))
}

fn utilization_ppm(cost: Duration, period: Duration) -> u32 {
    let denominator = period.as_nanos();
    if denominator == 0 {
        return ONE_MILLION_PPM;
    }
    let value = cost
        .as_nanos()
        .saturating_mul(u128::from(ONE_MILLION_PPM))
        .div_ceil(denominator);
    u32::try_from(value).unwrap_or(u32::MAX)
}

/// Outcome of one scheduled block.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PollExecutionOutcome {
    Read(Result<RawRegisters, BusError>),
    SkippedDeadline,
}

/// Bounded executor result tagged with the immutable plan version.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PollExecutionResult {
    plan_version: u64,
    block_index: u32,
    request_id: RequestId,
    completed_at: Instant,
    outcome: PollExecutionOutcome,
}

impl PollExecutionResult {
    #[must_use]
    pub const fn plan_version(&self) -> u64 {
        self.plan_version
    }

    #[must_use]
    pub const fn block_index(&self) -> u32 {
        self.block_index
    }

    #[must_use]
    pub const fn request_id(&self) -> RequestId {
        self.request_id
    }

    #[must_use]
    pub const fn completed_at(&self) -> Instant {
        self.completed_at
    }

    #[must_use]
    pub const fn outcome(&self) -> &PollExecutionOutcome {
        &self.outcome
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PollExecutorStatistics {
    pub plan_version: u64,
    pub plan_switches: u64,
    pub requests_started: u64,
    pub requests_completed: u64,
    pub deadlines_skipped: u64,
    pub results_dropped: u64,
}

#[derive(Debug, Error, Clone, Eq, PartialEq)]
pub enum PollExecutorError {
    #[error("poll result capacity must be non-zero")]
    ZeroResultCapacity,
    #[error("new poll plan version must be greater than the active version")]
    NonIncreasingPlanVersion,
    #[error("poll executor has stopped")]
    Stopped,
}

/// Handle for atomically replacing the plan between bus requests.
#[derive(Clone)]
pub struct PollExecutorHandle {
    plan: watch::Sender<Option<Arc<PollPlan>>>,
    stats: Arc<Mutex<PollExecutorStatistics>>,
}

impl PollExecutorHandle {
    pub fn update_plan(&self, plan: Arc<PollPlan>) -> Result<(), PollExecutorError> {
        let active_version = self
            .plan
            .borrow()
            .as_ref()
            .map_or(0, |active| active.version());
        if plan.version() <= active_version {
            return Err(PollExecutorError::NonIncreasingPlanVersion);
        }
        if self.plan.is_closed() {
            return Err(PollExecutorError::Stopped);
        }
        self.plan.send_replace(Some(Arc::clone(&plan)));
        let mut stats = lock_stats(&self.stats);
        stats.plan_version = plan.version();
        stats.plan_switches = stats.plan_switches.saturating_add(1);
        Ok(())
    }

    pub fn shutdown(&self) {
        self.plan.send_replace(None);
    }

    #[must_use]
    pub fn statistics(&self) -> PollExecutorStatistics {
        *lock_stats(&self.stats)
    }
}

pub struct PollExecutor;

impl PollExecutor {
    pub fn spawn(
        bus: Arc<dyn ReadBusPort>,
        clock: Arc<dyn MonotonicClock>,
        session_id: SessionId,
        initial_plan: Arc<PollPlan>,
        result_capacity: usize,
    ) -> Result<
        (
            PollExecutorHandle,
            mpsc::Receiver<PollExecutionResult>,
            JoinHandle<()>,
        ),
        PollExecutorError,
    > {
        if result_capacity == 0 {
            return Err(PollExecutorError::ZeroResultCapacity);
        }
        let (plan_tx, plan_rx) = watch::channel(Some(Arc::clone(&initial_plan)));
        let (result_tx, result_rx) = mpsc::channel(result_capacity);
        let stats = Arc::new(Mutex::new(PollExecutorStatistics {
            plan_version: initial_plan.version(),
            ..PollExecutorStatistics::default()
        }));
        let handle = PollExecutorHandle {
            plan: plan_tx,
            stats: Arc::clone(&stats),
        };
        let task = tokio::spawn(run_executor(
            bus, clock, session_id, plan_rx, result_tx, stats,
        ));
        Ok((handle, result_rx, task))
    }
}

#[derive(Clone)]
struct RuntimeBlock {
    block: PollBlock,
    next_due: Instant,
}

struct RuntimePlan {
    version: u64,
    blocks: Vec<RuntimeBlock>,
}

impl RuntimePlan {
    fn from_plan(plan: &PollPlan) -> Self {
        Self {
            version: plan.version(),
            blocks: plan
                .blocks()
                .iter()
                .cloned()
                .map(|block| RuntimeBlock {
                    next_due: block.next_due(),
                    block,
                })
                .collect(),
        }
    }

    fn earliest_index(&self) -> Option<usize> {
        self.blocks
            .iter()
            .enumerate()
            .min_by(|(_, left), (_, right)| {
                left.next_due
                    .cmp(&right.next_due)
                    .then_with(|| left.block.index().cmp(&right.block.index()))
            })
            .map(|(index, _)| index)
    }
}

async fn run_executor(
    bus: Arc<dyn ReadBusPort>,
    clock: Arc<dyn MonotonicClock>,
    session_id: SessionId,
    mut plan_rx: watch::Receiver<Option<Arc<PollPlan>>>,
    result_tx: mpsc::Sender<PollExecutionResult>,
    stats: Arc<Mutex<PollExecutorStatistics>>,
) {
    let Some(initial) = plan_rx.borrow().clone() else {
        return;
    };
    let mut runtime = RuntimePlan::from_plan(&initial);
    let mut next_request_id = 1_u64;

    loop {
        if plan_rx.has_changed().unwrap_or(false) {
            let plan = plan_rx.borrow_and_update().clone();
            let Some(plan) = plan else {
                return;
            };
            runtime = RuntimePlan::from_plan(&plan);
            continue;
        }
        let Some(index) = runtime.earliest_index() else {
            if plan_rx.changed().await.is_err() {
                return;
            }
            let plan = plan_rx.borrow_and_update().clone();
            let Some(plan) = plan else {
                return;
            };
            runtime = RuntimePlan::from_plan(&plan);
            continue;
        };
        let due = runtime.blocks[index].next_due;
        tokio::select! {
            biased;
            changed = plan_rx.changed() => {
                if changed.is_err() {
                    return;
                }
                let plan = plan_rx.borrow_and_update().clone();
                let Some(plan) = plan else {
                    return;
                };
                runtime = RuntimePlan::from_plan(&plan);
            }
            () = clock.sleep_until(due) => {
                if plan_rx.has_changed().unwrap_or(false) {
                    continue;
                }
                let request_id = RequestId::new(next_request_id);
                next_request_id = next_request_id.saturating_add(1);
                let now = clock.now();
                let block = runtime.blocks[index].block.clone();
                let deadline = due.checked_add(block.maximum_age()).unwrap_or(due);
                let outcome = if now >= deadline {
                    {
                        let mut current = lock_stats(&stats);
                        current.deadlines_skipped = current.deadlines_skipped.saturating_add(1);
                    }
                    PollExecutionOutcome::SkippedDeadline
                } else {
                    let context = match BusRequestContext::periodic(
                        request_id,
                        session_id,
                        block.request_class(),
                        deadline,
                    ) {
                        Ok(context) => context,
                        Err(error) => {
                            let result = PollExecutionResult {
                                plan_version: runtime.version,
                                block_index: block.index(),
                                request_id,
                                completed_at: clock.now(),
                                outcome: PollExecutionOutcome::Read(Err(error)),
                            };
                            publish_result(&result_tx, &stats, result);
                            runtime.blocks[index].next_due = advance_without_burst(
                                due,
                                block.period(),
                                clock.now(),
                            );
                            continue;
                        }
                    };
                    {
                        let mut current = lock_stats(&stats);
                        current.requests_started = current.requests_started.saturating_add(1);
                    }
                    let request = match ReadBusRequest::periodic(
                        context,
                        block.slave(),
                        block.function(),
                        block.block(),
                    ) {
                        Ok(request) => request,
                        Err(error) => {
                            let result = PollExecutionResult {
                                plan_version: runtime.version,
                                block_index: block.index(),
                                request_id,
                                completed_at: clock.now(),
                                outcome: PollExecutionOutcome::Read(Err(error)),
                            };
                            publish_result(&result_tx, &stats, result);
                            runtime.blocks[index].next_due = advance_without_burst(
                                due,
                                block.period(),
                                clock.now(),
                            );
                            continue;
                        }
                    };
                    let value = bus.read(request).await;
                    {
                        let mut current = lock_stats(&stats);
                        current.requests_completed = current.requests_completed.saturating_add(1);
                    }
                    PollExecutionOutcome::Read(value)
                };
                let completed_at = clock.now();
                publish_result(
                    &result_tx,
                    &stats,
                    PollExecutionResult {
                        plan_version: runtime.version,
                        block_index: block.index(),
                        request_id,
                        completed_at,
                        outcome,
                    },
                );
                runtime.blocks[index].next_due =
                    advance_without_burst(due, block.period(), completed_at);
            }
        }
    }
}

fn publish_result(
    sender: &mpsc::Sender<PollExecutionResult>,
    stats: &Arc<Mutex<PollExecutorStatistics>>,
    result: PollExecutionResult,
) {
    if sender.try_send(result).is_err() {
        let mut current = lock_stats(stats);
        current.results_dropped = current.results_dropped.saturating_add(1);
    }
}

fn advance_without_burst(due: Instant, period: Duration, now: Instant) -> Instant {
    if now < due {
        return due;
    }
    let elapsed = now.duration_since(due).as_nanos();
    let period_nanos = period.as_nanos().max(1);
    let steps = elapsed / period_nanos + 1;
    let nanos = period_nanos.saturating_mul(steps);
    due.checked_add(Duration::from_nanos(
        u64::try_from(nanos).unwrap_or(u64::MAX),
    ))
    .unwrap_or(now)
}

fn lock_stats(
    stats: &Arc<Mutex<PollExecutorStatistics>>,
) -> std::sync::MutexGuard<'_, PollExecutorStatistics> {
    stats
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeSet, VecDeque},
        sync::{Arc, Mutex},
        time::{Duration, Instant},
    };

    use lantern_domain::{
        BaudRate, DataBits, LinkSettings, ParameterId, Parity, RawRegisters, Rs485Mode, SessionId,
        SlaveId, StopBits,
    };
    use lantern_profile::{ProfileFormat, ValidatedDeviceProfile, parse_and_validate_profile};
    use proptest::prelude::*;

    use crate::{
        BusError, BusFuture, ManualMonotonicClock, MonotonicClock, ReadBusPort, ReadBusRequest,
        RequestClass,
    };

    use super::{
        FrequencyClass, PollCadences, PollExecutionOutcome, PollExecutor, PollExecutorError,
        PollPlanner, PollPlannerConfig, PollRejectionReason, ReadSubscription, SubscriberId,
        SubscriptionReason, estimate_read_transaction_cost,
    };

    #[derive(Clone, Copy)]
    struct ParameterSpec {
        address: u16,
        do_not_bridge: bool,
        maximum_bridge_gap: u16,
    }

    fn profile(specs: &[ParameterSpec], baud: u32) -> ValidatedDeviceProfile {
        let mut source = format!(
            r#"schema_version = 1
profile_id = "test.poll"
revision = 1
vendor = "Test"
family = "Planner"
model = "Synthetic"

[protocol]
default_baud_rate = {baud}
allowed_baud_rates = [{baud}]
default_parity = "none"
allowed_parities = ["none"]
default_data_bits = 8
allowed_data_bits = [8]
default_stop_bits = 1
allowed_stop_bits = [1]
response_timeout_ms = 100
default_slave_id = 1
rs485_mode = "adapter_managed"
"#,
        );
        for (index, spec) in specs.iter().enumerate() {
            source.push_str(&format!(
                r#"
[[parameters]]
id = "p{index:05}"
code = "P{index:05}"
name = "Parameter {index}"
table = "holding_registers"
address = {{ notation = "pdu_zero_based", value = {} }}
encoding = "unsigned16"
quantity = "frequency"
unit = "hz"
do_not_bridge = {}
maximum_bridge_gap = {}
scale = {{ multiplier = "1", divisor = "1", offset = "0", decimal_places = 0 }}
"#,
                spec.address, spec.do_not_bridge, spec.maximum_bridge_gap
            ));
        }
        parse_and_validate_profile(source.as_bytes(), ProfileFormat::Toml).expect("profile")
    }

    fn parameter(index: usize) -> ParameterId {
        ParameterId::parse(format!("p{index:05}")).expect("parameter ID")
    }

    fn subscriber(value: &str) -> SubscriberId {
        SubscriberId::parse(value).expect("subscriber")
    }

    fn subscription(
        index: usize,
        frequency: FrequencyClass,
        subscriber_id: &str,
        reason: SubscriptionReason,
        maximum_age: Duration,
    ) -> ReadSubscription {
        ReadSubscription::new(
            parameter(index),
            frequency,
            subscriber(subscriber_id),
            reason,
            false,
            maximum_age,
        )
        .expect("subscription")
    }

    fn link(baud: u32, parity: Parity, stop_bits: StopBits) -> LinkSettings {
        LinkSettings {
            baud_rate: BaudRate::new(baud).expect("baud"),
            parity,
            data_bits: DataBits::Eight,
            stop_bits,
            response_timeout: Duration::from_millis(100),
            slave_id: SlaveId::new(1).expect("slave"),
            rs485_mode: Rs485Mode::AdapterManaged,
        }
    }

    fn config(profile: &ValidatedDeviceProfile, cadences: PollCadences) -> PollPlannerConfig {
        PollPlannerConfig::new(
            cadences,
            profile.protocol().default_link(),
            Duration::from_millis(2),
            Duration::from_millis(1),
            700_000,
        )
        .expect("config")
    }

    #[test]
    fn deduplicates_parameter_and_preserves_fan_out() {
        let profile = profile(
            &[ParameterSpec {
                address: 0,
                do_not_bridge: false,
                maximum_bridge_gap: 0,
            }],
            115_200,
        );
        let now = Instant::now();
        let plan = PollPlanner::new()
            .build(
                &profile,
                [
                    subscription(
                        0,
                        FrequencyClass::Normal,
                        "dashboard",
                        SubscriptionReason::Dashboard,
                        Duration::from_secs(2),
                    ),
                    subscription(
                        0,
                        FrequencyClass::Normal,
                        "csv",
                        SubscriptionReason::Csv,
                        Duration::from_secs(2),
                    ),
                ],
                config(&profile, PollCadences::default()),
                now,
            )
            .expect("plan");
        assert_eq!(plan.blocks().len(), 1);
        assert_eq!(plan.blocks()[0].parameters().len(), 1);
        assert_eq!(plan.blocks()[0].parameters()[0].subscribers().len(), 2);
        assert_eq!(plan.blocks()[0].request_class(), RequestClass::Telemetry);
    }

    #[test]
    fn plan_is_independent_of_registration_order() {
        let profile = profile(
            &[
                ParameterSpec {
                    address: 0,
                    do_not_bridge: false,
                    maximum_bridge_gap: 1,
                },
                ParameterSpec {
                    address: 2,
                    do_not_bridge: false,
                    maximum_bridge_gap: 1,
                },
            ],
            115_200,
        );
        let subscriptions = vec![
            subscription(
                0,
                FrequencyClass::Normal,
                "scope",
                SubscriptionReason::Scope,
                Duration::from_secs(2),
            ),
            subscription(
                1,
                FrequencyClass::Normal,
                "dashboard",
                SubscriptionReason::Dashboard,
                Duration::from_secs(2),
            ),
        ];
        let mut reversed = subscriptions.clone();
        reversed.reverse();
        let now = Instant::now();
        let left = PollPlanner::new()
            .build(
                &profile,
                subscriptions,
                config(&profile, PollCadences::default()),
                now,
            )
            .expect("left");
        let right = PollPlanner::new()
            .build(
                &profile,
                reversed,
                config(&profile, PollCadences::default()),
                now,
            )
            .expect("right");
        assert_eq!(left, right);
    }

    #[test]
    fn fault_polling_is_critical_but_never_safety() {
        let profile = profile(
            &[ParameterSpec {
                address: 0,
                do_not_bridge: false,
                maximum_bridge_gap: 0,
            }],
            115_200,
        );
        let plan = PollPlanner::new()
            .build(
                &profile,
                [subscription(
                    0,
                    FrequencyClass::Fast,
                    "fault-tracker",
                    SubscriptionReason::Fault,
                    Duration::from_millis(500),
                )],
                config(&profile, PollCadences::default()),
                Instant::now(),
            )
            .expect("plan");
        assert_eq!(
            plan.blocks()[0].request_class(),
            RequestClass::TelemetryCritical
        );
        assert_ne!(
            plan.blocks()[0].request_class(),
            RequestClass::SafetyOneShot
        );
    }

    #[test]
    fn bridge_gap_and_do_not_bridge_are_enforced() {
        let bridgeable = profile(
            &[
                ParameterSpec {
                    address: 0,
                    do_not_bridge: false,
                    maximum_bridge_gap: 1,
                },
                ParameterSpec {
                    address: 2,
                    do_not_bridge: false,
                    maximum_bridge_gap: 1,
                },
            ],
            115_200,
        );
        let subscriptions = [
            subscription(
                0,
                FrequencyClass::Normal,
                "a",
                SubscriptionReason::Dashboard,
                Duration::from_secs(2),
            ),
            subscription(
                1,
                FrequencyClass::Normal,
                "b",
                SubscriptionReason::Dashboard,
                Duration::from_secs(2),
            ),
        ];
        let joined = PollPlanner::new()
            .build(
                &bridgeable,
                subscriptions.clone(),
                config(&bridgeable, PollCadences::default()),
                Instant::now(),
            )
            .expect("joined");
        assert_eq!(joined.blocks().len(), 1);
        assert_eq!(joined.blocks()[0].block().count().get(), 3);

        let barred = profile(
            &[
                ParameterSpec {
                    address: 0,
                    do_not_bridge: true,
                    maximum_bridge_gap: 1,
                },
                ParameterSpec {
                    address: 2,
                    do_not_bridge: false,
                    maximum_bridge_gap: 1,
                },
            ],
            115_200,
        );
        let separated = PollPlanner::new()
            .build(
                &barred,
                subscriptions,
                config(&barred, PollCadences::default()),
                Instant::now(),
            )
            .expect("separated");
        assert_eq!(separated.blocks().len(), 2);
    }

    #[test]
    fn grouping_never_crosses_modbus_read_limit() {
        let profile = profile(
            &[
                ParameterSpec {
                    address: 0,
                    do_not_bridge: false,
                    maximum_bridge_gap: 124,
                },
                ParameterSpec {
                    address: 125,
                    do_not_bridge: false,
                    maximum_bridge_gap: 124,
                },
            ],
            115_200,
        );
        let plan = PollPlanner::new()
            .build(
                &profile,
                [
                    subscription(
                        0,
                        FrequencyClass::Slow,
                        "a",
                        SubscriptionReason::Diagnostics,
                        Duration::from_secs(20),
                    ),
                    subscription(
                        1,
                        FrequencyClass::Slow,
                        "b",
                        SubscriptionReason::Diagnostics,
                        Duration::from_secs(20),
                    ),
                ],
                config(&profile, PollCadences::default()),
                Instant::now(),
            )
            .expect("plan");
        assert_eq!(plan.blocks().len(), 2);
        assert!(
            plan.blocks()
                .iter()
                .all(|block| block.block().count().get() <= 125)
        );
    }

    #[test]
    fn maximum_age_upgrades_requested_frequency() {
        let profile = profile(
            &[ParameterSpec {
                address: 0,
                do_not_bridge: false,
                maximum_bridge_gap: 0,
            }],
            115_200,
        );
        let plan = PollPlanner::new()
            .build(
                &profile,
                [subscription(
                    0,
                    FrequencyClass::Slow,
                    "dashboard",
                    SubscriptionReason::Dashboard,
                    Duration::from_millis(1_500),
                )],
                config(&profile, PollCadences::default()),
                Instant::now(),
            )
            .expect("plan");
        assert_eq!(plan.blocks()[0].frequency(), FrequencyClass::Normal);
    }

    #[test]
    fn on_demand_only_does_not_create_a_periodic_timer() {
        let profile = profile(
            &[ParameterSpec {
                address: 0,
                do_not_bridge: false,
                maximum_bridge_gap: 0,
            }],
            115_200,
        );
        let plan = PollPlanner::new()
            .build(
                &profile,
                [subscription(
                    0,
                    FrequencyClass::OnDemand,
                    "browser",
                    SubscriptionReason::ParameterBrowser,
                    Duration::from_secs(1),
                )],
                config(&profile, PollCadences::default()),
                Instant::now(),
            )
            .expect("plan");
        assert!(plan.blocks().is_empty());
        assert_eq!(
            plan.rejections()[0].reason(),
            PollRejectionReason::OnDemandOnly
        );
    }

    #[test]
    fn over_budget_plan_is_degraded_or_rejected_fail_closed() {
        let profile = profile(
            &[
                ParameterSpec {
                    address: 0,
                    do_not_bridge: true,
                    maximum_bridge_gap: 0,
                },
                ParameterSpec {
                    address: 10,
                    do_not_bridge: true,
                    maximum_bridge_gap: 0,
                },
                ParameterSpec {
                    address: 20,
                    do_not_bridge: true,
                    maximum_bridge_gap: 0,
                },
            ],
            9_600,
        );
        let cadences = PollCadences::new(
            Duration::from_millis(10),
            Duration::from_millis(100),
            Duration::from_secs(1),
        )
        .expect("cadences");
        let config = PollPlannerConfig::new(
            cadences,
            profile.protocol().default_link(),
            Duration::from_millis(20),
            Duration::from_millis(5),
            20_000,
        )
        .expect("config");
        let plan = PollPlanner::new()
            .build(
                &profile,
                (0..3).map(|index| {
                    subscription(
                        index,
                        FrequencyClass::Fast,
                        &format!("consumer-{index}"),
                        SubscriptionReason::Diagnostics,
                        Duration::from_secs(2),
                    )
                }),
                config,
                Instant::now(),
            )
            .expect("plan");
        assert!(plan.utilization_ppm() <= plan.budget_ppm());
        assert!(!plan.degradations().is_empty() || !plan.rejections().is_empty());
    }

    #[test]
    fn cost_uses_serial_character_format_and_t35() {
        let profile = profile(
            &[ParameterSpec {
                address: 0,
                do_not_bridge: false,
                maximum_bridge_gap: 0,
            }],
            9_600,
        );
        let block = profile.parameter(&parameter(0)).expect("parameter").block();
        let eight_n_one = estimate_read_transaction_cost(
            link(9_600, Parity::None, StopBits::One),
            block,
            Duration::ZERO,
            Duration::ZERO,
            Duration::ZERO,
        );
        let eight_e_two = estimate_read_transaction_cost(
            link(9_600, Parity::Even, StopBits::Two),
            block,
            Duration::ZERO,
            Duration::ZERO,
            Duration::ZERO,
        );
        assert!(eight_e_two > eight_n_one);
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(32))]

        #[test]
        fn arbitrary_unique_addresses_never_duplicate_parameters_or_exceed_125(
            addresses in prop::collection::btree_set(0_u16..400_u16, 1..40),
        ) {
            let specs = addresses
                .iter()
                .map(|address| ParameterSpec {
                    address: *address,
                    do_not_bridge: false,
                    maximum_bridge_gap: 3,
                })
                .collect::<Vec<_>>();
            let profile = profile(&specs, 115_200);
            let plan = PollPlanner::new()
                .build(
                    &profile,
                    (0..specs.len()).map(|index| {
                        subscription(
                            index,
                            FrequencyClass::Slow,
                            &format!("p-{index}"),
                            SubscriptionReason::Diagnostics,
                            Duration::from_secs(20),
                        )
                    }),
                    config(&profile, PollCadences::default()),
                    Instant::now(),
                )
                .expect("plan");
            let mut seen = BTreeSet::new();
            for block in plan.blocks() {
                prop_assert!(block.block().count().get() <= 125);
                for slice in block.parameters() {
                    prop_assert!(seen.insert(slice.parameter_id().clone()));
                }
            }
            prop_assert!(plan.utilization_ppm() <= plan.budget_ppm());
        }
    }

    #[derive(Default)]
    struct FakeBus {
        requests: Mutex<Vec<ReadBusRequest>>,
        responses: Mutex<VecDeque<Result<RawRegisters, BusError>>>,
    }

    impl FakeBus {
        fn with_successes(count: usize) -> Self {
            Self {
                requests: Mutex::new(Vec::new()),
                responses: Mutex::new(
                    (0..count)
                        .map(|_| RawRegisters::new(vec![7]).map_err(|_| BusError::InvalidResponse))
                        .collect(),
                ),
            }
        }

        fn requests(&self) -> Vec<ReadBusRequest> {
            self.requests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    impl ReadBusPort for FakeBus {
        fn read(&self, request: ReadBusRequest) -> BusFuture<'static, RawRegisters> {
            self.requests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(request);
            let response = self
                .responses
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .pop_front()
                .unwrap_or_else(|| {
                    RawRegisters::new(vec![7]).map_err(|_| BusError::InvalidResponse)
                });
            Box::pin(async move { response })
        }
    }

    async fn next_result(
        receiver: &mut tokio::sync::mpsc::Receiver<super::PollExecutionResult>,
    ) -> super::PollExecutionResult {
        tokio::time::timeout(Duration::from_secs(1), receiver.recv())
            .await
            .expect("executor result timeout")
            .expect("executor result channel")
    }

    #[tokio::test]
    async fn executor_skips_a_suspended_cycle_without_catch_up_burst() {
        let profile = profile(
            &[ParameterSpec {
                address: 0,
                do_not_bridge: false,
                maximum_bridge_gap: 0,
            }],
            115_200,
        );
        let clock = Arc::new(ManualMonotonicClock::new());
        let cadences = PollCadences::new(
            Duration::from_millis(100),
            Duration::from_millis(100),
            Duration::from_millis(100),
        )
        .expect("cadences");
        let plan = Arc::new(
            PollPlanner::new()
                .build(
                    &profile,
                    [subscription(
                        0,
                        FrequencyClass::Fast,
                        "dashboard",
                        SubscriptionReason::Dashboard,
                        Duration::from_millis(150),
                    )],
                    config(&profile, cadences),
                    clock.now() + Duration::from_millis(100),
                )
                .expect("plan"),
        );
        let bus = Arc::new(FakeBus::with_successes(4));
        let (handle, mut receiver, task) =
            PollExecutor::spawn(bus.clone(), clock.clone(), SessionId::new(1), plan, 4)
                .expect("executor");
        tokio::task::yield_now().await;
        clock.advance(Duration::from_secs(1));
        let result = next_result(&mut receiver).await;
        assert_eq!(result.outcome(), &PollExecutionOutcome::SkippedDeadline);
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert!(receiver.try_recv().is_err());
        assert!(bus.requests().is_empty());
        assert_eq!(handle.statistics().deadlines_skipped, 1);
        handle.shutdown();
        task.await.expect("executor");
    }

    #[tokio::test]
    async fn plan_switch_cancels_unsent_old_version() {
        let profile = profile(
            &[
                ParameterSpec {
                    address: 0,
                    do_not_bridge: false,
                    maximum_bridge_gap: 0,
                },
                ParameterSpec {
                    address: 10,
                    do_not_bridge: false,
                    maximum_bridge_gap: 0,
                },
            ],
            115_200,
        );
        let planner = PollPlanner::new();
        let clock = Arc::new(ManualMonotonicClock::new());
        let now = clock.now() + Duration::from_millis(100);
        let first = Arc::new(
            planner
                .build(
                    &profile,
                    [subscription(
                        0,
                        FrequencyClass::Normal,
                        "old",
                        SubscriptionReason::Dashboard,
                        Duration::from_secs(2),
                    )],
                    config(&profile, PollCadences::default()),
                    now,
                )
                .expect("first"),
        );
        let second = Arc::new(
            planner
                .build(
                    &profile,
                    [subscription(
                        1,
                        FrequencyClass::Normal,
                        "new",
                        SubscriptionReason::Dashboard,
                        Duration::from_secs(2),
                    )],
                    config(&profile, PollCadences::default()),
                    now,
                )
                .expect("second"),
        );
        let bus = Arc::new(FakeBus::with_successes(2));
        let (handle, mut receiver, task) =
            PollExecutor::spawn(bus.clone(), clock.clone(), SessionId::new(1), first, 4)
                .expect("executor");
        handle.update_plan(second.clone()).expect("plan update");
        tokio::task::yield_now().await;
        clock.advance(Duration::from_millis(100));
        let result = next_result(&mut receiver).await;
        assert_eq!(result.plan_version(), second.version());
        assert_eq!(bus.requests().len(), 1);
        assert_eq!(bus.requests()[0].block().start().get(), 10);
        handle.shutdown();
        task.await.expect("executor");
    }

    #[tokio::test]
    async fn result_backlog_is_bounded_and_reported() {
        let profile = profile(
            &[ParameterSpec {
                address: 0,
                do_not_bridge: false,
                maximum_bridge_gap: 0,
            }],
            115_200,
        );
        let clock = Arc::new(ManualMonotonicClock::new());
        let cadences = PollCadences::new(
            Duration::from_millis(10),
            Duration::from_millis(10),
            Duration::from_millis(10),
        )
        .expect("cadences");
        let plan = Arc::new(
            PollPlanner::new()
                .build(
                    &profile,
                    [subscription(
                        0,
                        FrequencyClass::Fast,
                        "csv",
                        SubscriptionReason::Csv,
                        Duration::from_secs(1),
                    )],
                    config(&profile, cadences),
                    clock.now() + Duration::from_millis(10),
                )
                .expect("plan"),
        );
        let bus = Arc::new(FakeBus::with_successes(20));
        let (handle, _receiver, task) =
            PollExecutor::spawn(bus, clock.clone(), SessionId::new(1), plan, 1).expect("executor");
        for _ in 0..5 {
            tokio::task::yield_now().await;
            clock.advance(Duration::from_millis(10));
            for _ in 0..5 {
                tokio::task::yield_now().await;
            }
        }
        assert!(handle.statistics().results_dropped > 0);
        handle.shutdown();
        task.await.expect("executor");
    }

    #[test]
    fn plan_versions_must_increase() {
        let profile = profile(
            &[ParameterSpec {
                address: 0,
                do_not_bridge: false,
                maximum_bridge_gap: 0,
            }],
            115_200,
        );
        let plan = Arc::new(
            PollPlanner::new()
                .build(
                    &profile,
                    [subscription(
                        0,
                        FrequencyClass::Normal,
                        "dashboard",
                        SubscriptionReason::Dashboard,
                        Duration::from_secs(2),
                    )],
                    config(&profile, PollCadences::default()),
                    Instant::now(),
                )
                .expect("plan"),
        );
        let clock: Arc<dyn MonotonicClock> = Arc::new(ManualMonotonicClock::new());
        let bus: Arc<dyn ReadBusPort> = Arc::new(FakeBus::default());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            let (handle, _receiver, task) =
                PollExecutor::spawn(bus, clock, SessionId::new(1), plan.clone(), 1)
                    .expect("executor");
            assert_eq!(
                handle.update_plan(plan),
                Err(PollExecutorError::NonIncreasingPlanVersion)
            );
            handle.shutdown();
            task.await.expect("executor");
        });
    }
}
