use serde_json::{Value, json};

use super::{CommandSpec, can_redo, can_undo, cmd};
use crate::Session;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!("edit.undo", "Undo", ["Edit"], Some("Cmd+Z"), "{} → {undone, warnings?}", can_undo, |s, _| {
            s.end_interaction()?;
            let undone = s.undo_step()?;
            Ok(step_result(s, json!({"undone": undone})))
        }),
        cmd!("edit.redo", "Redo", ["Edit"], Some("Cmd+Shift+Z"), "{} → {redone, warnings?}", can_redo, |s, _| {
            let redone = s.redo_step()?;
            Ok(step_result(s, json!({"redone": redone})))
        }),
    ]
}

/// An undo or redo step's result, with `warnings` naming what its folder move across volumes
/// could not remove from the old place.
fn step_result(s: &mut Session, mut result: Value) -> Value {
    let left = std::mem::take(&mut s.folder_left_behind);
    if !left.is_empty() {
        result["warnings"] = json!(left);
    }
    result
}
