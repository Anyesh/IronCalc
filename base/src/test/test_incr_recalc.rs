#![allow(clippy::unwrap_used)]

// The incremental recalc against a reference model that takes the same
// edits: the values must match after every recalc, and a recalc runs the
// reference only when the plan says it must.

use crate::lockstep::snapshot;
use crate::test::util::new_empty_model;
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

    fn content_nodes(&self) -> usize {
        let state = self.incremental.incr.as_ref().unwrap();
        let mut count = 0;
        state.content.for_each(&state.rt, |_, _| count += 1);
        count
    }

    fn rebuilds(&self) -> u32 {
        self.incremental.incr.as_ref().unwrap().rebuilds.get()
    }

    fn text(&self, cell: &str) -> String {
        self.incremental._get_text(cell)
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
    assert!(pair.content_nodes() < 10, "{}", pair.content_nodes());
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
