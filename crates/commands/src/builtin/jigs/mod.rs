//! Jigs and placement (production short list #2, #8, #9, #15): Fit to Stock (`fit`), jigs with numbered pockets
//! and designs placed in every pocket (`jig`), the pre-flight checks next to Start (`preflight`) and the region check
//! before re-marking (`remark`). See ARCHITECTURE.md "Jigs and placement".

mod fit;
mod jig;
mod preflight;
mod remark;
#[cfg(test)]
mod links_tests;

pub(crate) use jig::{placement_cells, refuse_placement};

use crate::Registry;

/// Register this module's commands.
pub(crate) fn register(r: &mut Registry) {
    r.register::<fit::FitToStock>();
    r.register::<jig::JigCreate>();
    r.register::<jig::JigFromObjects>();
    r.register::<jig::JigImport>();
    r.register::<jig::JigEdit>();
    r.register::<jig::JigMove>();
    r.register::<jig::JigSetPockets>();
    r.register::<jig::JigAddKeepOut>();
    r.register::<jig::JigRemoveKeepOut>();
    r.register::<jig::JigDelete>();
    r.register::<jig::JigPlace>();
    r.register::<jig::JigList>();
    r.register::<preflight::Preflight>();
    r.register::<preflight::PreflightSet>();
    r.register::<remark::RemarkCheck>();
}
