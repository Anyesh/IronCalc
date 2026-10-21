#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]

use crate::calc_result::CalcResult;
#[cfg(debug_assertions)]
use crate::evaluation::SheetRead;
use crate::expressions::types::CellReferenceIndex;
use crate::incr::ctx;
use crate::incr::nodes::{tile_of, Constant, Content};
use crate::model::Model;
use crate::test::util::new_empty_model;
use crate::types::{ArrayKind, Cell, FormulaValue, SpillValue};

#[test]
fn incremental_is_off_until_switched_on() {
    let mut model = new_empty_model();
    assert!(!model.is_incremental());
    model.set_incremental(true);
    assert!(model.is_incremental());
    model.set_incremental(false);
    assert!(!model.is_incremental());
}

#[test]
fn switching_on_keeps_the_shipped_results() {
    let mut model = new_empty_model();
    model.set_incremental(true);
    model._set("A1", "2");
    model._set("A2", "=A1*21");
    model.evaluate();
    assert_eq!(model._get_text("A2"), "42");
}

#[test]
fn content_drops_the_style_and_the_formula_result() {
    let unevaluated = Cell::CellFormula {
        f: 3,
        s: 0,
        v: FormulaValue::Unevaluated,
    };
    let evaluated = Cell::CellFormula {
        f: 3,
        s: 5,
        v: FormulaValue::Number(42.0),
    };
    assert_eq!(
        Content::of(Some(&unevaluated), &[]),
        Content::of(Some(&evaluated), &[])
    );
    assert_eq!(
        Content::of(Some(&Cell::NumberCell { v: 7.0, s: 2 }), &[]),
        Content::Constant(Constant::Number(7.0))
    );
    assert_eq!(
        Content::of(Some(&Cell::EmptyCell { s: 4 }), &[]),
        Content::Blank
    );
    assert_eq!(Content::of(None, &[]), Content::Empty);
    let spill = Cell::SpillCell {
        s: 1,
        a: (1, 2),
        v: SpillValue::Number(3.0),
    };
    assert_eq!(
        Content::of(Some(&spill), &[]),
        Content::Spill {
            anchor: (1, 2),
            pinned: None
        }
    );
}

#[test]
fn content_resolves_shared_strings() {
    let strings = ["a".to_string(), "b".to_string()];
    let cell = Cell::SharedString { si: 1, s: 0 };
    assert_eq!(
        Content::of(Some(&cell), &strings),
        Content::Constant(Constant::Text("b".to_string()))
    );
    let missing = Cell::SharedString { si: 9, s: 0 };
    assert_eq!(
        Content::of(Some(&missing), &strings),
        Content::Constant(Constant::MissingString)
    );
}

#[test]
fn content_keeps_an_anchor_and_its_area() {
    let anchor = Cell::ArrayFormula {
        f: 0,
        s: 0,
        r: (2, 3),
        kind: ArrayKind::Dynamic,
        v: FormulaValue::Number(1.0),
    };
    assert_eq!(
        Content::of(Some(&anchor), &[]),
        Content::Formula {
            f: 0,
            array: Some((ArrayKind::Dynamic, (2, 3))),
            pinned: None
        }
    );
}

#[test]
fn tiles_are_64_by_64_blocks() {
    assert_eq!(tile_of((0, 1, 1)), tile_of((0, 63, 63)));
    assert_ne!(tile_of((0, 63, 1)), tile_of((0, 64, 1)));
    assert_ne!(tile_of((0, 1, 63)), tile_of((0, 1, 64)));
    assert_ne!(tile_of((0, 1, 1)), tile_of((1, 1, 1)));
}

#[test]
fn eval_ctx_hands_out_the_armed_model_and_restores_on_drop() {
    let mut model = new_empty_model();
    assert!(!ctx::is_armed());
    {
        let _outer = ctx::arm(&mut model);
        ctx::with_model(|outer| {
            outer.workbook.name = "outer".to_string();
            let _inner = ctx::arm(outer);
            ctx::with_model(|inner| inner.workbook.name.push_str(" and inner"));
        });
        assert!(ctx::is_armed());
        ctx::with_model(|outer| assert_eq!(outer.workbook.name, "outer and inner"));
    }
    assert!(!ctx::is_armed());
    assert_eq!(model.workbook.name, "outer and inner");
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "read the sheet without a node")]
fn a_compute_reading_the_sheet_directly_panics() {
    let mut model = new_empty_model();
    model._set("A1", "3");
    let _armed = ctx::arm(&mut model);
    ctx::with_model(|model| model._get_text("A1"));
}

#[test]
fn a_compute_may_read_the_sheet_a_node_accounts_for() {
    let mut model = new_empty_model();
    model._set("A1", "3");
    model.set_incremental(true);
    let _armed = ctx::arm(&mut model);
    let text = ctx::with_model(|model| {
        let _read = model.note_content_read(model._parse_reference("A1"));
        model._get_text("A1")
    });
    assert_eq!(text, "3");
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "read the sheet without a node")]
fn a_nested_compute_does_not_inherit_its_callers_allowance() {
    let mut model = new_empty_model();
    model._set("A1", "3");
    let _armed = ctx::arm(&mut model);
    ctx::with_model(|outer| {
        let _read = SheetRead::exempt();
        let _inner = ctx::arm(outer);
        ctx::with_model(|inner| inner._get_text("A1"));
    });
}

#[test]
fn reading_the_sheet_outside_a_compute_is_not_audited() {
    let mut model = new_empty_model();
    model._set("A1", "3");
    model.set_incremental(true);
    assert_eq!(model._get_text("A1"), "3");
}

#[test]
#[should_panic(expected = "EVAL_CTX is not armed")]
fn eval_ctx_unarmed_panics() {
    ctx::with_model(|_| ());
}

/// Every cell of the first sheet, as the reference left it.
fn cells_of(model: &Model) -> Vec<(CellReferenceIndex, Cell)> {
    model.workbook.worksheets[0]
        .sheet_data
        .cells()
        .map(|(row, column, cell)| {
            let p = CellReferenceIndex {
                sheet: 0,
                row,
                column,
            };
            (p, cell.into_owned())
        })
        .collect()
}

/// Exactly the same value, errors with their origin and message included.
fn same(a: &CalcResult, b: &CalcResult) -> bool {
    match (a, b) {
        (CalcResult::Number(a), CalcResult::Number(b)) => a.to_bits() == b.to_bits(),
        (CalcResult::String(a), CalcResult::String(b)) => a == b,
        (CalcResult::Boolean(a), CalcResult::Boolean(b)) => a == b,
        (CalcResult::EmptyCell, CalcResult::EmptyCell) => true,
        (
            CalcResult::Error {
                error,
                origin,
                message,
            },
            CalcResult::Error {
                error: other_error,
                origin: other_origin,
                message: other_message,
            },
        ) => error == other_error && origin == other_origin && message == other_message,
        _ => false,
    }
}

/// Evaluates incrementally, then checks that the read path gives each cell
/// what the reference stored: a pinned formula reads its stored value.
fn assert_matches_reference(model: &mut Model) {
    model.set_incremental(true);
    model.evaluate();
    for (p, _) in cells_of(model) {
        let stored = model.stored_value(p);
        assert!(same(&model.incr_cell_value(p), &stored), "at {p:?}");
    }
}

#[test]
fn incremental_values_equal_the_reference() {
    let mut model = new_empty_model();
    for (cell, input) in [
        ("A1", "1"),
        ("A2", "2.5"),
        ("A3", "text"),
        ("A4", "TRUE"),
        ("A5", "#N/A"),
        ("B1", "=A1+A2"),
        ("B2", "=B1*2"),
        ("B3", "=CONCATENATE(A3,\"x\")"),
        ("B4", "=IF(A4,1,0)"),
        ("B5", "=A5"),
        ("B6", "=1/0"),
        ("B7", "=SUM(A1:A5)"),
        ("B8", "=A99"),
        ("B9", "=VLOOKUP(99,A1:B2,2,FALSE)"),
        ("B10", "=LET(x,A1,x*3)"),
        ("F1", "=SUM(D1:E2)"),
        ("G1", "=SEQUENCE(3)"),
        ("H1", "=SUM(G1:G3)"),
        ("I1", "=SUM(A:A)"),
    ] {
        model._set(cell, input);
    }
    model
        .set_user_array_formula(0, 1, 4, 2, 2, "=A1:B2*2")
        .unwrap();
    assert_matches_reference(&mut model);
}

#[test]
fn a_cycle_falls_back_to_circ() {
    let mut model = new_empty_model();
    model._set("A1", "=A2");
    model._set("A2", "=A1+1");
    model._set("A3", "=A1*2");
    assert_matches_reference(&mut model);
    assert_eq!(model._get_text("A1"), "#CIRC!");
}

#[test]
fn a_chain_deeper_than_the_frame_budget_replays() {
    let mut model = new_empty_model();
    model._set("A1", "1");
    for row in 2..=300 {
        model._set(&format!("A{row}"), &format!("=A{}+1", row - 1));
    }
    let top = CellReferenceIndex {
        sheet: 0,
        row: 300,
        column: 1,
    };
    model.set_incremental(true);
    assert_eq!(model.incr_value(top).v, FormulaValue::Number(300.0));
    assert_matches_reference(&mut model);
}

/// Changes a cell and tells the incremental evaluator what changed: the
/// cell's content, its tile and the sheet's used area.
fn edit(model: &mut Model, cell: &str, input: &str) {
    model._set(cell, input);
    let p = model._parse_reference(cell);
    let state = model.incr.clone().unwrap();
    let rt = &state.rt;
    let key = (p.sheet, p.row, p.column);
    if let Some(handle) = state.content.get(rt, crate::incr::grid(key)) {
        rt.set(handle, model.content_on_sheet(p));
    }
    if let Some(handle) = state.tile.get(rt, &tile_of(key)) {
        rt.set(handle, rt.get(handle) + 1);
    }
    let d = model.workbook.worksheets[p.sheet as usize].dimension();
    state.set_dimension(p.sheet, (d.min_row, d.max_row, d.min_column, d.max_column));
}

fn incr_text(model: &mut Model, cell: &str) -> FormulaValue {
    let p = model._parse_reference(cell);
    model.incr_value(p).v
}

fn number(n: f64) -> FormulaValue {
    FormulaValue::Number(n)
}

#[test]
fn a_range_of_constants_depends_on_its_tiles() {
    let mut model = new_empty_model();
    model._set("A1", "1");
    model._set("A2", "2");
    model._set("C1", "=SUMPRODUCT(A1:A2)");
    model.set_incremental(true);
    assert_eq!(incr_text(&mut model, "C1"), number(3.0));
    edit(&mut model, "A2", "5");
    assert_eq!(incr_text(&mut model, "C1"), number(6.0));
}

#[test]
fn a_row_held_as_numbers_depends_on_its_tiles() {
    let mut model = new_empty_model();
    model._set("A1", "1");
    model._set("B1", "2");
    model._set("D1", "1");
    model._set("D2", "1");
    model._set("C3", "=MMULT(A1:B1,D1:D2)");
    model.set_incremental(true);
    assert_eq!(incr_text(&mut model, "C3"), number(3.0));
    edit(&mut model, "B1", "5");
    assert_eq!(incr_text(&mut model, "C3"), number(6.0));
}

#[test]
fn a_whole_column_depends_on_the_used_area() {
    let mut model = new_empty_model();
    model._set("A1", "1");
    model._set("C1", "=SUM(A:A)");
    model.set_incremental(true);
    assert_eq!(incr_text(&mut model, "C1"), number(1.0));
    edit(&mut model, "A100", "2");
    assert_eq!(incr_text(&mut model, "C1"), number(3.0));
}

#[test]
fn sumif_over_whole_columns_depends_on_the_used_area() {
    let mut model = new_empty_model();
    model._set("A1", "1");
    model._set("C1", "=SUMIF(A:A,\">0\")");
    model.set_incremental(true);
    assert_eq!(incr_text(&mut model, "C1"), number(1.0));
    edit(&mut model, "A100", "2");
    assert_eq!(incr_text(&mut model, "C1"), number(3.0));
}

#[test]
fn isformula_and_formulatext_depend_on_the_content() {
    let mut model = new_empty_model();
    model._set("A1", "1");
    model._set("B1", "=ISFORMULA(A1)");
    model._set("C1", "=FORMULATEXT(A2)");
    model._set("A2", "=1+1");
    model.set_incremental(true);
    assert_eq!(incr_text(&mut model, "B1"), FormulaValue::Boolean(false));
    assert_eq!(
        incr_text(&mut model, "C1"),
        FormulaValue::Text("=1+1".to_string())
    );
    edit(&mut model, "A1", "=1");
    // the same value in another formula
    edit(&mut model, "A2", "=4-2");
    assert_eq!(incr_text(&mut model, "B1"), FormulaValue::Boolean(true));
    assert_eq!(
        incr_text(&mut model, "C1"),
        FormulaValue::Text("=4-2".to_string())
    );
}

#[test]
fn subtotal_depends_on_which_cells_are_subtotals() {
    let mut model = new_empty_model();
    model._set("A1", "1");
    model._set("A2", "=SUBTOTAL(9,A1)");
    model._set("B1", "=SUBTOTAL(9,A1:A2)");
    model.set_incremental(true);
    assert_eq!(incr_text(&mut model, "B1"), number(1.0));
    // the same value, but no longer a subtotal
    edit(&mut model, "A2", "=SUM(A1)");
    assert_eq!(incr_text(&mut model, "B1"), number(2.0));
}

#[test]
fn volatile_cells_share_the_reference_draws_and_rerun_on_a_new_epoch() {
    let mut model = new_empty_model();
    model._set("A1", "=RAND()");
    model._set("B1", "=NOW()");
    model.set_incremental(true);
    crate::mock_time::set_mock_time(1_700_000_000_000);
    for _ in 0..2 {
        model.evaluate();
        for cell in ["A1", "B1"] {
            let Cell::CellFormula { v, .. } = model._get_cell(cell) else {
                panic!("{cell} is not a formula");
            };
            assert_eq!(incr_text(&mut model, cell), v, "{cell}");
        }
        let state = model.incr.clone().unwrap();
        state.rt.set(state.epoch, state.rt.get(state.epoch) + 1);
        crate::mock_time::set_mock_time(1_700_086_400_000);
    }
}

#[test]
fn a_run_that_attaches_no_link_drops_the_one_an_earlier_run_attached() {
    let mut model = new_empty_model();
    model._set("B1", "TRUE");
    model._set(
        "A1",
        "=IF(B1,HYPERLINK(\"https://example.com\",\"x\"),\"no\")",
    );
    model.set_incremental(true);
    incr_text(&mut model, "A1");
    assert!(model.links.contains_key(&(0, 1, 1)));
    edit(&mut model, "B1", "FALSE");
    assert_eq!(
        incr_text(&mut model, "A1"),
        FormulaValue::Text("no".to_string())
    );
    assert!(!model.links.contains_key(&(0, 1, 1)));
}

#[test]
fn a_run_of_a_formula_draws_what_its_earlier_runs_drew() {
    let a1 = CellReferenceIndex {
        sheet: 0,
        row: 1,
        column: 1,
    };
    let mut model = new_empty_model();
    model.set_incremental(true);
    model.start_formula_run(a1);
    let first = (model.draw_random(a1), model.draw_random(a1));
    assert_ne!(first.0, first.1);
    model.start_formula_run(a1);
    assert_eq!((model.draw_random(a1), model.draw_random(a1)), first);
    // a new recalc draws afresh
    model.volatile.clear();
    model.start_formula_run(a1);
    assert_ne!(model.draw_random(a1), first.0);
}

#[test]
fn the_frame_budget_is_one_when_the_engine_unwinds_at_every_formula() {
    use crate::incr::{frame_budget, MAX_FRAMES};
    assert_eq!(frame_budget(1), 1);
    assert_eq!(frame_budget(64), MAX_FRAMES);
    assert_eq!(frame_budget(usize::MAX), MAX_FRAMES);
    assert_eq!(MAX_FRAMES, if cfg!(debug_assertions) { 16 } else { 64 });
}

#[test]
fn the_budgets_follow_the_evaluation_limits() {
    let budgets = |model: &Model| model.incr.as_ref().unwrap().budgets();
    let mut model = new_empty_model();
    model.set_max_evaluation_depth(1);
    model.set_max_evaluation_stack(1000);
    model.set_incremental(true);
    assert_eq!(budgets(&model), (1, 1000));
    model.set_max_evaluation_depth(64);
    assert_eq!(budgets(&model), (crate::incr::MAX_FRAMES, 1000));
    model.set_max_evaluation_stack(256 * 1024);
    assert_eq!(budgets(&model), (crate::incr::MAX_FRAMES, 256 * 1024));
}

/// The reference's nested-function chain (`long_reverse_chains_with_the_default_limits`),
/// 64 deep, on a 2 MiB thread.
#[test]
fn a_chain_of_nested_formulas_fits_a_small_thread() {
    std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(|| {
            let mut model = new_empty_model();
            for row in 1..64 {
                model._set(
                    &format!("B{row}"),
                    &format!("=IF(TRUE,SUM(1,IF(TRUE,B{}+0,0)),0)", row + 1),
                );
            }
            model._set("B64", "1");
            assert_matches_reference(&mut model);
            assert_eq!(model._get_text("B1"), "64");
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn a_dynamic_anchor_depends_on_the_tiles_of_its_area() {
    let mut model = new_empty_model();
    model._set("A1", "=SEQUENCE(3)");
    model.evaluate();
    model.set_incremental(true);
    assert_eq!(incr_text(&mut model, "A1"), number(1.0));
    edit(&mut model, "A3", "x");
    assert!(matches!(
        incr_text(&mut model, "A1"),
        FormulaValue::Error { .. }
    ));
}

#[cfg(target_pointer_width = "64")]
#[test]
fn a_content_node_is_a_text_constant_and_a_tag() {
    assert_eq!(
        std::mem::size_of::<Content>(),
        std::mem::size_of::<Constant>() + std::mem::size_of::<usize>(),
        "every constant's content slot pays for the widest variant and a tag"
    );
}
