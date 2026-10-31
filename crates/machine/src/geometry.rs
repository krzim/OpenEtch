//! [`field_to_galvo`]: field-frame mm -> galvo words for one machine profile.

use lmc::LmcError;

use crate::profile::MachineProfile;

/// A point that cannot be sent to the galvos.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum FieldError {
    #[error("point ({x_mm}, {y_mm}) is not finite")]
    NonFinite { x_mm: f64, y_mm: f64 },
    /// Outside the open square `-half..+half` on either axis.
    #[error("point ({x_mm:.3}, {y_mm:.3}) mm is outside the {field_mm} mm field (|x|, |y| must be < {:.3} mm)", field_mm / 2.0)]
    OutsideField { x_mm: f64, y_mm: f64, field_mm: f64 },
}

/// Field-frame mm (origin at the field centre, +X right, +Y away from the operator) -> galvo `(x, y)` words.
///
/// A thin wrapper over `lmc::units::field_to_galvo` with the profile's [`MachineProfile::field_transform`], the one
/// transform `plan` encodes jobs with too, so a frame shows exactly where the job will mark. Order: refuse points
/// not strictly inside `±field_mm / 2` on both axes; negate X and/or Y in the field frame (`negate_x`,
/// `negate_y`); swap the axes (`swap_xy`); then `mm_to_galvo` per axis. Lens correction is not applied here: the
/// board applies the uploaded .cor table itself.
///
/// The bound is strict on both sides (even though `-field/2` is encodable) so the accepted region is the same
/// whatever the negate settings.
pub fn field_to_galvo(profile: &MachineProfile, x_mm: f64, y_mm: f64) -> Result<(u16, u16), FieldError> {
    match lmc::units::field_to_galvo(x_mm, y_mm, &profile.field_transform()) {
        Ok(words) => Ok(words),
        Err(LmcError::NonFinite { .. }) => Err(FieldError::NonFinite { x_mm, y_mm }),
        // A validated profile has a positive field, so anything else is the bound.
        Err(_) => Err(FieldError::OutsideField { x_mm, y_mm, field_mm: profile.field_mm }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> MachineProfile {
        MachineProfile { field_mm: 153.75, ..MachineProfile::generic_lmc_fiber() }
    }

    #[test]
    fn centre_and_axes() {
        let p = profile();
        assert_eq!(field_to_galvo(&p, 0.0, 0.0), Ok((0x8000, 0x8000)));
        let (x, y) = field_to_galvo(&p, 10.0, -5.0).unwrap();
        assert!(x > 0x8000 && y < 0x8000);
        assert_eq!(x, lmc::units::mm_to_galvo(10.0, 153.75).unwrap());
    }

    #[test]
    fn negate_then_swap() {
        let mut p = profile();
        let plain = field_to_galvo(&p, 10.0, -5.0).unwrap();
        p.negate_x = true;
        let neg = field_to_galvo(&p, 10.0, -5.0).unwrap();
        assert_eq!(neg, (field_to_galvo(&profile(), -10.0, -5.0).unwrap().0, plain.1));
        p.swap_xy = true;
        let swapped = field_to_galvo(&p, 10.0, -5.0).unwrap();
        assert_eq!(swapped, (neg.1, neg.0));
    }

    #[test]
    fn field_bounds_refused() {
        let p = profile();
        let half = 153.75 / 2.0;
        assert!(field_to_galvo(&p, half - 0.001, -(half - 0.001)).is_ok());
        for (x, y) in [(half, 0.0), (-half, 0.0), (0.0, half), (0.0, -half), (100.0, 0.0), (0.0, -1e9)] {
            assert!(matches!(field_to_galvo(&p, x, y), Err(FieldError::OutsideField { .. })), "({x}, {y})");
        }
        assert!(matches!(field_to_galvo(&p, f64::NAN, 0.0), Err(FieldError::NonFinite { .. })));
        assert!(matches!(field_to_galvo(&p, 0.0, f64::INFINITY), Err(FieldError::NonFinite { .. })));
    }
}
