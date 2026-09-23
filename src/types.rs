use serde::Serialize;

/// What kind of sensor produced the reading. Cheap to copy, exhaustive
/// to match on — the enum-for-message-types idiom from the spec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ReadingKind {
    Ultrasonic,
    Imu,
}

/// One simulated sensor sample. `seq` is the source-assigned monotonic
/// sequence number the deduplicator reasons about.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SensorReading {
    pub seq: u64,
    /// Nanoseconds since pipeline start, assigned by the source.
    pub ts_ns: u64,
    pub kind: ReadingKind,
    /// Interpreted per `kind`: 1 value for ultrasonic (cm), 6 for IMU
    /// (accel xyz + gyro xyz).
    pub values: Vec<f32>,
}

/// What the source pushes downstream. `Disconnected` plays the role
/// bidi-rig gave to stream termination events.
#[derive(Debug, Clone, PartialEq)]
pub enum SourceEvent {
    Reading(SensorReading),
    Disconnected,
}