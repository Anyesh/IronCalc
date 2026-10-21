#![allow(clippy::unwrap_used)]

// The incremental recalc against a reference model that takes the same
// edits: the values must match after every recalc, and a recalc runs the
// reference only when the plan says it must.

use crate::expressions::types::Area;
use crate::lockstep::snapshot;
use crate::test::util::new_empty_model;
use crate::types::SheetState;
use crate::Model;

struct Pair {
    incremental: Model<'static>,
    reference: Model<'static>,
}

impl Pair {
    fn new() -> Pair {
        let mut incremental = new_empty_model();
        incremental.set_incremental(true);
        Pair {
            incremental,
            reference: new_empty_model(),
        }
    }

    /// Makes the edit in both models and evaluates them.
    fn set(&mut self, cell: &str, input: &str) {
        self.incremental._set(cell, input);
        self.reference._set(cell, input);
        self.evaluate();
    }

    /// Removes the cell from both models and evaluates them.
    fn remove(&mut self, cell: &str) {
        let p = self.incremental._parse_reference(cell);
        let area = Area {
            sheet: p.sheet,
            row: p.row,
            column: p.column,
            width: 1,
            height: 1,
        };
        self.incremental.range_clear_all(&area).unwrap();
        self.reference.range_clear_all(&area).unwrap();
        self.evaluate();
    }

    fn evaluate(&mut self) {
        self.incremental.evaluate();
        self.reference.evaluate();
        assert_eq!(snapshot(&self.incremental), snapshot(&self.reference));
    }

    fn reference_runs(&self) -> u32 {
        self.incremental.incr.as_ref().unwrap().reference_runs.get()
    }

    fn tile_reads(&self) -> u64 {
        self.incremental.incr.as_ref().unwrap().tile_reads.get()
    }

    fn used_area_scans(&self) -> u64 {
        self.incremental
            .incr
            .as_ref()
            .unwrap()
            .used_area_scans
            .get()
    }

    fn deferred(&self) -> usize {
        self.incremental
            .incr
            .as_ref()
            .unwrap()
            .deferred
            .borrow()
            .len()
    }

    fn anchor_scans(&self) -> u64 {
        self.incremental.incr.as_ref().unwrap().anchor_scans.get()
    }

    fn computes(&self) -> u64 {
        self.incremental.incr.as_ref().unwrap().computes.get()
    }

    /// The cells of the first sheet's `column`, rows 1 to `rows`, that
    /// have a `content` node.
    fn content_nodes(&self, column: i32, rows: i32) -> usize {
        let state = self.incremental.incr.as_ref().unwrap();
        (1..=rows)
            .filter(|&row| {
                let key = crate::incr::grid((0, row, column));
                state.content.get(&state.rt, key).is_some()
            })
            .count()
    }

    fn rebuilds(&self) -> u32 {
        self.incremental.incr.as_ref().unwrap().rebuilds.get()
    }

    fn text(&self, cell: &str) -> String {
        self.incremental._get_text(cell)
    }

    /// Makes the edit in both models, evaluates them, and asserts that the
    /// incremental one rebuilt its state for it.
    fn rebuilds_after(&mut self, edit: impl Fn(&mut Model<'static>)) {
        let rebuilds = self.rebuilds();
        edit(&mut self.incremental);
        edit(&mut self.reference);
        self.evaluate();
        assert_eq!(self.rebuilds(), rebuilds + 1);
    }
}

#[test]
fn an_edit_recomputes_its_dependents_without_the_reference() {
    let mut pair = Pair::new();
    pair.set("A1", "1");
    pair.set("A2", "=A1*2");
    pair.set("A3", "=A2+A1");
    assert_eq!(pair.text("A3"), "3");
    let before = pair.reference_runs();
    pair.set("A1", "5");
    assert_eq!(pair.text("A3"), "15");
    pair.set("A2", "=A1*3");
    assert_eq!(pair.text("A3"), "20");
    assert_eq!(pair.reference_runs(), before);
}

#[test]
fn the_first_evaluate_builds_the_nodes_once() {
    let mut pair = Pair::new();
    pair.incremental._set("A1", "2");
    pair.incremental._set("B1", "=A1+1");
    pair.reference._set("A1", "2");
    pair.reference._set("B1", "=A1+1");
    pair.evaluate();
    assert_eq!(pair.rebuilds(), 1);
    pair.set("A1", "3");
    assert_eq!(pair.text("B1"), "4");
    assert_eq!(pair.rebuilds(), 1);
}

#[test]
fn a_cleared_formula_is_empty_to_its_readers() {
    let mut pair = Pair::new();
    pair.set("A1", "=1+1");
    pair.set("B1", "=A1*10");
    assert_eq!(pair.text("B1"), "20");
    pair.set("A1", "");
    assert_eq!(pair.text("B1"), "0");
    pair.set("A1", "=2+2");
    assert_eq!(pair.text("B1"), "40");
}

#[test]
fn a_reader_of_a_formula_depends_on_its_value_alone() {
    let mut pair = Pair::new();
    pair.set("A1", "=1+1");
    pair.set("B1", "=A1*10");
    let computes = pair.computes();
    pair.set("A1", "=4/2");
    assert_eq!(pair.text("B1"), "20");
    assert_eq!(pair.computes(), computes + 1);
}

#[test]
fn a_range_reader_sees_a_new_cell_in_its_range() {
    let mut pair = Pair::new();
    pair.set("A1", "1");
    pair.set("B1", "=SUM(A1:A5)");
    pair.set("A4", "10");
    assert_eq!(pair.text("B1"), "11");
    pair.set("C1", "=SUM(A:A)");
    pair.set("A20", "100");
    assert_eq!(pair.text("C1"), "111");
}

#[test]
fn a_cse_array_writes_its_area() {
    let mut pair = Pair::new();
    pair.set("A1", "1");
    pair.set("A2", "2");
    pair.incremental
        .set_user_array_formula(0, 1, 2, 1, 2, "=A1:A2*10")
        .unwrap();
    pair.reference
        .set_user_array_formula(0, 1, 2, 1, 2, "=A1:A2*10")
        .unwrap();
    pair.evaluate();
    pair.set("A2", "7");
    assert_eq!(pair.text("B2"), "70");
}

#[test]
fn a_new_dynamic_anchor_runs_the_reference_and_an_edit_elsewhere_does_not() {
    let mut pair = Pair::new();
    pair.set("A1", "3");
    let before = pair.reference_runs();
    pair.set("B1", "=SEQUENCE(A1)");
    assert_eq!(pair.reference_runs(), before + 1);
    assert_eq!(pair.text("B3"), "3");
    // Outside the tiles of the anchor's area, which the anchor reads.
    pair.set("D100", "5");
    assert_eq!(pair.reference_runs(), before + 1);
    pair.set("A1", "4");
    assert_eq!(pair.text("B4"), "4");
    assert_eq!(pair.reference_runs(), before + 2);
}

#[test]
fn an_anchor_that_reads_its_own_area_restarts_every_reference() {
    let mut pair = Pair::new();
    pair.set("B4", "=B5:B7");
    assert_eq!(pair.text("B4"), "#CIRC!");
    let runs = pair.reference_runs();
    // The restart log's marks last one evaluation, so the shipped engine
    // restarts on B4 again and the recalc must count that restart.
    pair.set("D100", "5");
    assert_eq!(pair.reference_runs(), runs + 1);
    pair.set("B4", "1");
    pair.set("D100", "6");
    assert_eq!(pair.reference_runs(), runs + 2);
}

#[test]
fn a_compute_reads_each_tile_of_a_range_once() {
    let mut pair = Pair::new();
    pair.set("A1", "1");
    let before = pair.tile_reads();
    pair.set("B1", "=ROWS(TRIMRANGE(A1:A6400))");
    assert_eq!(pair.text("B1"), "1");
    // Rows 1 to 6400 lie in tile rows 0 to 100.
    assert_eq!(pair.tile_reads() - before, 101);
}

#[test]
fn a_read_outside_the_used_area_needs_no_node_per_cell() {
    let mut pair = Pair::new();
    pair.set("A1", "1");
    pair.set("A2", "3");
    pair.set("B1", "=COVARIANCE.S(A1:A5000,A1:A5000)");
    assert_eq!(pair.text("B1"), "2");
    let nodes = pair.content_nodes(1, 5000);
    assert!(nodes <= 2, "{nodes}");
    pair.set("A3", "5");
    assert_eq!(pair.text("B1"), "4");
}

#[test]
fn a_zero_that_changes_sign_is_a_change() {
    let mut pair = Pair::new();
    pair.set("A1", "1");
    pair.set("B1", "=A1*0");
    pair.set("C1", "=B1");
    pair.set("A1", "-1");
    pair.set("D1", "0");
    pair.set("E1", "=D1");
    pair.set("D1", "-0");
}

#[test]
fn a_cycle_runs_the_reference_and_matches_it() {
    let mut pair = Pair::new();
    pair.set("A1", "=B1+1");
    pair.set("B1", "=A1+1");
    pair.set("C1", "=A1*2");
    assert_eq!(pair.text("A1"), "#CIRC!");
    let runs = pair.reference_runs();
    assert!(runs > 0);
    pair.set("D1", "7");
    assert_eq!(pair.reference_runs(), runs);
    pair.set("B1", "1");
    assert_eq!(pair.text("C1"), "4");
}

#[test]
fn a_structural_edit_is_followed() {
    let mut pair = Pair::new();
    pair.set("A1", "1");
    pair.set("A2", "=A1+1");
    pair.incremental.insert_rows(0, 1, 1).unwrap();
    pair.reference.insert_rows(0, 1, 1).unwrap();
    pair.evaluate();
    assert_eq!(pair.text("A3"), "2");
    pair.set("A2", "5");
    assert_eq!(pair.text("A3"), "6");
}

#[test]
fn deleting_the_last_anchor_drops_it_from_the_order() {
    let mut pair = Pair::new();
    pair.set("A1", "3");
    pair.set("B2", "=SEQUENCE(A1)");
    pair.incremental.delete_rows(0, 2, 1).unwrap();
    pair.reference.delete_rows(0, 2, 1).unwrap();
    pair.evaluate();
}

#[test]
fn a_renamed_sheet_rebuilds() {
    let mut pair = Pair::new();
    pair.set("A1", "1");
    let rebuilds = pair.rebuilds();
    pair.incremental.rename_sheet("Sheet1", "Data").unwrap();
    pair.reference.rename_sheet("Sheet1", "Data").unwrap();
    pair.evaluate();
    assert_eq!(pair.rebuilds(), rebuilds + 1);
}

#[test]
fn a_recalc_with_nothing_changed_computes_nothing() {
    let mut pair = Pair::new();
    pair.set("A1", "1");
    pair.set("A2", "=A1+1");
    let runs = pair.reference_runs();
    let rebuilds = pair.rebuilds();
    pair.evaluate();
    pair.evaluate();
    assert_eq!((pair.reference_runs(), pair.rebuilds()), (runs, rebuilds));
}

#[test]
fn hiding_a_row_rebuilds() {
    let mut pair = Pair::new();
    pair.set("A1", "1");
    pair.set("A2", "2");
    pair.set("A3", "=SUBTOTAL(109,A1:A2)");
    pair.rebuilds_after(|model| model.set_row_hidden(0, 2, true).unwrap());
    assert_eq!(pair.text("A3"), "1");
}

#[test]
fn hiding_a_hidden_row_again_does_not_rebuild() {
    let mut pair = Pair::new();
    pair.set("A1", "1");
    pair.set("A2", "=A1+1");
    pair.rebuilds_after(|model| model.set_row_hidden(0, 2, true).unwrap());
    let rebuilds = pair.rebuilds();
    pair.incremental.set_row_hidden(0, 2, true).unwrap();
    pair.reference.set_row_hidden(0, 2, true).unwrap();
    pair.evaluate();
    assert_eq!(pair.rebuilds(), rebuilds);
}

#[test]
fn inserting_rows_above_a_hidden_row_rebuilds() {
    let mut pair = Pair::new();
    pair.set("A1", "1");
    pair.set("B1", "=SUBTOTAL(109,A1:A10)");
    pair.rebuilds_after(|model| model.set_row_hidden(0, 5, true).unwrap());
    pair.rebuilds_after(|model| model.insert_rows(0, 3, 1).unwrap());
    pair.rebuilds_after(|model| model.delete_rows(0, 3, 1).unwrap());
    pair.set("A5", "4");
    assert_eq!(pair.text("B1"), "1");
}

#[test]
fn a_merge_rebuilds() {
    let mut pair = Pair::new();
    pair.set("A1", "=SEQUENCE(1,3)");
    let area = Area {
        sheet: 0,
        row: 2,
        column: 2,
        width: 2,
        height: 1,
    };
    pair.rebuilds_after(|model| model.merge_cells(&area).unwrap());
    pair.rebuilds_after(|model| model.unmerge_cells(&area).unwrap());
}

#[test]
fn a_defined_name_edit_rebuilds() {
    let mut pair = Pair::new();
    pair.set("B1", "1");
    pair.set("B2", "2");
    pair.rebuilds_after(|model| model.new_defined_name("x", None, "Sheet1!$B$1").unwrap());
    pair.set("A1", "=x*2");
    pair.rebuilds_after(|model| {
        model
            .update_defined_name("x", None, "x", None, "Sheet1!$B$2")
            .unwrap()
    });
    assert_eq!(pair.text("A1"), "4");
}

#[test]
fn a_settings_or_sheet_state_edit_rebuilds() {
    let mut pair = Pair::new();
    pair.set("A1", "1");
    pair.rebuilds_after(|model| model.set_timezone("Europe/Berlin").unwrap());
    pair.rebuilds_after(|model| model.set_sheet_state(0, SheetState::Hidden).unwrap());
}

/// The metadata a recalc does not compare is trusted to change only through
/// the model's own mutators; a test build checks that it did.
#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "metadata changed without metadata_edited")]
fn a_hidden_row_set_behind_the_models_back_is_caught() {
    let mut model = new_empty_model();
    model.set_incremental(true);
    model._set("A1", "=SUBTOTAL(109,A2:A3)");
    model.evaluate();
    model.workbook.worksheets[0]
        .set_row_hidden(2, true)
        .unwrap();
    model.evaluate();
}

#[test]
fn an_edit_scans_the_used_area_only_when_it_may_shrink() {
    let mut pair = Pair::new();
    pair.set("A1", "1");
    pair.set("B1", "=A1+1");
    pair.set("A1", "2");
    assert_eq!(pair.used_area_scans(), 0, "nothing reads the used area");
    pair.set("C1", "=SUM(A:A)");
    pair.set("A5", "5");
    pair.set("B3", "3");
    pair.remove("B3");
    assert_eq!(pair.used_area_scans(), 0, "growing, or removing inside");
    pair.remove("A5");
    assert_eq!(pair.used_area_scans(), 1, "removing the last row");
    pair.set("A7", "7");
    assert_eq!(pair.text("C1"), "9");
}

/// B(r) = B(r - 1) + A(r) over `rows` rows, with D1 = E1 + A1 and E1 = D1, a
/// cycle that reads A1: once it is pinned, every edit of A1 runs the
/// reference.
fn running_sum_with_cycle(rows: i32) -> Pair {
    let mut pair = Pair::new();
    for row in 1..=rows {
        pair.set(&format!("A{row}"), &row.to_string());
        let formula = if row == 1 {
            "=A1".to_string()
        } else {
            format!("=B{}+A{row}", row - 1)
        };
        pair.set(&format!("B{row}"), &formula);
    }
    pair.set("D1", "=E1+A1");
    pair.set("E1", "=D1");
    pair
}

#[test]
fn a_reference_recalc_leaves_the_cone_it_reached_to_the_next_recalc() {
    let mut pair = running_sum_with_cycle(20);
    let runs = pair.reference_runs();
    pair.set("A1", "100");
    assert_eq!(pair.reference_runs(), runs + 1);
    assert_eq!(pair.deferred(), 20, "B1:B20 are the reference's");
    pair.set("A1", "200");
    assert_eq!(pair.reference_runs(), runs + 2);
    assert_eq!(pair.deferred(), 20);
    pair.set("A20", "7");
    assert_eq!(pair.reference_runs(), runs + 2);
    assert_eq!(pair.deferred(), 0);
    assert_eq!(pair.text("B20"), "396");
}

#[test]
fn a_reference_recalc_with_no_anchor_does_not_scan_for_anchors() {
    let mut pair = running_sum_with_cycle(5);
    let scans = pair.anchor_scans();
    pair.set("A1", "100");
    assert_eq!(pair.anchor_scans(), scans);
    pair.set("C1", "=A1:A3");
    assert_eq!(
        pair.anchor_scans(),
        scans + 1,
        "the reference made an anchor"
    );
    pair.set("A2", "9");
    assert_eq!(pair.text("C2"), "9");
}

#[test]
fn a_cycle_the_reference_finds_among_deferred_formulas_is_pinned_now() {
    let mut pair = running_sum_with_cycle(5);
    pair.set("G1", "=IF(A1>150,H1,0)");
    pair.set("H1", "=G1");
    let runs = pair.reference_runs();
    pair.set("A1", "200");
    assert_eq!(pair.reference_runs(), runs + 1);
    pair.set("A5", "9");
    assert_eq!(
        pair.reference_runs(),
        runs + 1,
        "the edit reads no pin: G1 and H1 were pinned by the last resync"
    );
}
