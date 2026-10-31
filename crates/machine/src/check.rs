//! [`check_job`]: the profile's hard limits, checked on the encoded packets right before they are sent.

use lmc::{ListCommand, Packet, units};

use crate::profile::MachineProfile;

/// Why a job's packets were refused. Nothing has been sent when this is returned.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum JobCheckError {
    #[error("the job has no packets")]
    Empty,
    #[error("the job's list has no end-of-list")]
    Unterminated,
    #[error("slot {slot}: mark current {current} is above the profile's {max_power_pct} % cap ({cap})")]
    PowerAboveCap { slot: usize, current: u16, cap: u16, max_power_pct: f64 },
    #[error("slot {slot}: Q-switch period {period} is outside the profile's frequency range ({min_period}..={max_period})")]
    Frequency { slot: usize, period: u16, min_period: u16, max_period: u16 },
    #[error("slot {slot}: pulse width {ns} ns is not in the profile's table")]
    PulseWidth { slot: usize, ns: u16 },
    #[error("slot {slot}: mark speed {units} galvo units/ms is 0 or above the profile's limit ({max_units})")]
    MarkSpeed { slot: usize, units: u16, max_units: u16 },
    /// Only typed `lmc::ListCommand` variants may be sent; raw opcodes could set power some other way.
    #[error("slot {slot}: opcode {opcode:#06x} is not allowed in a job")]
    UnsupportedCommand { slot: usize, opcode: u16 },
}

/// Check every slot of every packet (padding and anything after the end-of-list included) against `profile`:
/// mark current <= the `max_power_pct` cap, Q-switch period within the frequency range, pulse width in the table,
/// mark speed in 1..=the max mark speed, and no `ListCommand::Other`. The list must also end with an end-of-list.
pub fn check_job(profile: &MachineProfile, packets: &[Packet]) -> Result<(), JobCheckError> {
    if packets.is_empty() {
        return Err(JobCheckError::Empty);
    }
    if lmc::decode(packets).last() != Some(&ListCommand::EndOfList) {
        return Err(JobCheckError::Unterminated);
    }
    // The profile is validated, so these conversions succeed; fall back to the most restrictive value otherwise.
    let cap = units::power_to_mark_current(profile.max_power_pct).unwrap_or(0);
    let min_period = units::frequency_to_period(profile.frequency_khz_max).unwrap_or(u16::MAX);
    let max_period = units::frequency_to_period(profile.frequency_khz_min).unwrap_or(0);
    let max_units = units::speed_to_units(profile.mark_speed.max_mm_s, profile.field_mm).unwrap_or(0);
    for (slot, words) in packets.iter().flat_map(Packet::words).enumerate() {
        match ListCommand::from_words(words) {
            ListCommand::MarkCurrent { current } if current > cap => {
                return Err(JobCheckError::PowerAboveCap { slot, current, cap, max_power_pct: profile.max_power_pct });
            }
            ListCommand::QSwitchPeriod { period } if !(min_period..=max_period).contains(&period) => {
                return Err(JobCheckError::Frequency { slot, period, min_period, max_period });
            }
            ListCommand::FiberPulseWidth { ns } if !profile.pulse_widths_ns.contains(&ns) => {
                return Err(JobCheckError::PulseWidth { slot, ns });
            }
            ListCommand::MarkSpeed { units } if units == 0 || units > max_units => {
                return Err(JobCheckError::MarkSpeed { slot, units, max_units });
            }
            ListCommand::Other { words } => return Err(JobCheckError::UnsupportedCommand { slot, opcode: words[0] }),
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lmc::MarkingList;

    fn profile() -> MachineProfile {
        MachineProfile { field_mm: 153.75, max_power_pct: 50.0, ..MachineProfile::generic_lmc_fiber() }
    }

    fn job(edit: impl FnOnce(&mut MarkingList)) -> Vec<Packet> {
        let mut l = MarkingList::new();
        edit(&mut l);
        l.jump_to(30000, 30000).mark_to(31000, 30000);
        l.finish()
    }

    #[test]
    fn within_limits_passes() {
        let p = profile();
        let packets = job(|l| {
            l.mark_power(units::power_to_mark_current(50.0).unwrap()).unwrap();
            l.q_switch_period(units::frequency_to_period(30.0).unwrap()).unwrap();
            l.fiber_pulse_width(200).unwrap();
            l.mark_speed(units::speed_to_units(1000.0, 153.75).unwrap()).unwrap();
        });
        assert_eq!(check_job(&p, &packets), Ok(()));
    }

    #[test]
    fn limits_refused() {
        let p = profile();
        let over = units::power_to_mark_current(50.1).unwrap();
        assert!(matches!(check_job(&p, &job(|l| _ = l.mark_power(over))), Err(JobCheckError::PowerAboveCap { .. })));
        let slow = units::frequency_to_period(0.5).unwrap();
        assert!(matches!(check_job(&p, &job(|l| _ = l.q_switch_period(slow))), Err(JobCheckError::Frequency { .. })));
        assert!(matches!(check_job(&p, &job(|l| _ = l.fiber_pulse_width(7))), Err(JobCheckError::PulseWidth { .. })));
        let fast = units::speed_to_units(20000.0, 153.75).unwrap();
        assert!(matches!(check_job(&p, &job(|l| _ = l.mark_speed(fast))), Err(JobCheckError::MarkSpeed { .. })));
        let raw = job(|l| _ = l.push(ListCommand::Other { words: [lmc::op::LIST_MARK_POWER_RATIO, 9, 0, 0, 0, 0] }));
        assert!(matches!(check_job(&p, &raw), Err(JobCheckError::UnsupportedCommand { .. })));
        assert_eq!(check_job(&p, &[]), Err(JobCheckError::Empty));
        let unterminated = Packet::from_words(&[ListCommand::ReadyMark.to_words(); 256]).unwrap();
        assert_eq!(check_job(&p, &[unterminated]), Err(JobCheckError::Unterminated));
    }
}
