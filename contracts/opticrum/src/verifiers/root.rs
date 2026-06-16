use ckb_cinnabar_verifier::{
    re_exports::ckb_std::{self, high_level::load_cell_data},
    this_script_args, Result, Verification,
};
use ckb_std::{ckb_constants::Source, debug, high_level::load_cell_lock};

use crate::{
    error::OpticrumError,
    utils::{find_opticrum_script, get_unoccupied_capacity, parse_xudt},
    Branch, Context, OpticrumPattern, OpticrumState,
};

/// Root verifier: inspects args length to determine Order vs Match state,
/// checks ScriptPattern, and routes to the appropriate branch.
#[derive(Default)]
pub struct Root;

impl Verification<Context> for Root {
    fn verify(&mut self, name: &str, ctx: &mut Context) -> Result<Option<&str>> {
        debug!("Entering [{name}]");

        // Prase input Opticrum state
        let args = this_script_args()?;
        let data = load_cell_data(0, Source::GroupInput)?;
        ctx.old_state.branch = Branch::parse(&args, &data)?;
        ctx.old_state.unoccupied_capacity = get_unoccupied_capacity(0, Source::GroupInput)?;
        ctx.old_state.xudt = parse_xudt(0, Source::GroupInput)?;

        // Parse output Opticrum state
        ctx.new_state = find_opticrum_script(Source::Output)
            .map(|index| {
                let args = load_cell_lock(index, Source::Output)?.args().raw_data();
                let data = load_cell_data(index, Source::Output)?;
                Result::<OpticrumState>::Ok(OpticrumState {
                    branch: Branch::parse(&args, &data)?,
                    unoccupied_capacity: get_unoccupied_capacity(index, Source::Output)?,
                    xudt: parse_xudt(index, Source::Output)?,
                })
            })
            .transpose()?;

        // Determine cell state by branch mode
        match ctx.old_state.compare(ctx.new_state.as_ref()) {
            OpticrumPattern::OrderCancel => Ok(Some("order_cancel")),
            OpticrumPattern::OrderMatch => Ok(Some("order_match")),
            OpticrumPattern::MatchExtract => Ok(Some("match_extract")),
            OpticrumPattern::MatchDestroy => Ok(Some("match_destroy")),
            _ => Err(OpticrumError::UnknownState.into()),
        }
    }
}
