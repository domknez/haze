//! AirPlay's volume scale: dB from -30.0 (quietest) to 0.0, with -144.0 for mute.
//! The sink sends it and the speaker's remote control reports it back, so the
//! curve and its inverse live together.

/// Mute, in AirPlay dB.
pub const MUTE_DB: f32 = -144.0;
/// The quietest audible level, in AirPlay dB.
const MIN_DB: f32 = -30.0;

/// A volume of 0.0 ..= 1.0 in AirPlay dB.
pub fn to_db(volume: f32) -> f32 {
    if volume <= 0.001 { MUTE_DB } else { MIN_DB - MIN_DB * volume.clamp(0.0, 1.0) }
}

/// AirPlay dB (mute, or -30 ..= 0) as a volume of 0.0 ..= 1.0.
pub fn from_db(db: f32) -> f32 {
    if db <= MIN_DB { 0.0 } else { ((db - MIN_DB) / -MIN_DB).clamp(0.0, 1.0) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ends_and_middle() {
        assert_eq!(to_db(0.0), MUTE_DB);
        assert_eq!(to_db(1.0), 0.0);
        assert_eq!(to_db(0.5), -15.0);
        assert_eq!(from_db(MUTE_DB), 0.0);
        assert_eq!(from_db(-30.0), 0.0);
        assert_eq!(from_db(0.0), 1.0);
        assert_eq!(from_db(3.0), 1.0, "louder than the scale is full volume");
    }

    #[test]
    fn round_trips() {
        for i in 0..=100 {
            let v = i as f32 / 100.0;
            assert!((from_db(to_db(v)) - v).abs() < 1e-5, "{v}");
        }
        for tenths in -300..=0 {
            let db = tenths as f32 / 10.0;
            assert!((to_db(from_db(db)) - db).abs() < 1e-4 || db == -30.0, "{db}");
        }
    }
}
