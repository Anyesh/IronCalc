//! The lockstep differential: a model with the incremental evaluator on and
//! a reference model with it off take the same edits, and after every step
//! their snapshots must be identical. Base's tests, xlsx's tests and the wasm
//! harness all drive it through here.

use std::fmt::Write as _;

use crate::expressions::types::{Area, CellReferenceIndex};
use crate::expressions::utils::number_to_column;
use crate::model::Model;
use crate::types::Cell;
use crate::user_model::UserModel;

/// Everything evaluation leaves behind, one line per item: every cell with
/// its value and error details (spill cells included), the dynamic links,
/// the conditional formatting results, the anchor order and the restart
/// count.
pub fn snapshot(model: &Model) -> Vec<String> {
    let mut lines = Vec::new();
    for (sheet, worksheet) in model.workbook.worksheets.iter().enumerate() {
        lines.push(format!("sheet {sheet} {:?}", worksheet.name));
        for (row, column, cell) in worksheet.sheet_data.cells() {
            lines.push(format!("{sheet}!{row},{column} {:?}", &*cell));
        }
    }
    let mut links: Vec<_> = model.links.iter().collect();
    links.sort_by_key(|(position, _)| **position);
    for (position, link) in links {
        lines.push(format!("link {position:?} {link:?}"));
    }
    let mut formats: Vec<_> = model.cf_cache.iter().collect();
    formats.sort_by_key(|(position, _)| **position);
    for (position, results) in formats {
        lines.push(format!("cf {position:?} {results:?}"));
    }
    lines.push(format!("anchor order {:?}", model.evaluation.anchor_order));
    lines.push(format!(
        "restarts {}",
        model.evaluation.restarts_in_last_evaluation
    ));
    lines
}

/// An edit, as the UI makes it.
#[derive(Clone, Debug)]
pub enum Edit {
    /// A constant or a formula; empty clears the cell.
    Input(u32, i32, i32, String),
    /// A CSE array formula: sheet, row, column, width, height, formula.
    Cse(u32, i32, i32, i32, i32, String),
    /// Sheet, row, column, width, height.
    ClearRange(u32, i32, i32, i32, i32),
    Undo,
    Redo,
    InsertRows(u32, i32, i32),
    DeleteRows(u32, i32, i32),
    InsertColumns(u32, i32, i32),
    DeleteColumns(u32, i32, i32),
    RenameSheet(u32, String),
    NewSheet,
    DefinedName(String, String),
    HideRows(u32, i32, i32, bool),
}

impl Edit {
    pub fn apply(&self, model: &mut UserModel) -> Result<(), String> {
        match self {
            Edit::Input(sheet, row, column, input) => {
                model.set_user_input(*sheet, *row, *column, input)
            }
            Edit::Cse(sheet, row, column, width, height, formula) => {
                model.set_user_array_formula(*sheet, *row, *column, *width, *height, formula)
            }
            Edit::ClearRange(sheet, row, column, width, height) => {
                model.range_clear_contents(&Area {
                    sheet: *sheet,
                    row: *row,
                    column: *column,
                    width: *width,
                    height: *height,
                })
            }
            Edit::Undo => model.undo(),
            Edit::Redo => model.redo(),
            Edit::InsertRows(sheet, row, count) => model.insert_rows(*sheet, *row, *count),
            Edit::DeleteRows(sheet, row, count) => model.delete_rows(*sheet, *row, *count),
            Edit::InsertColumns(sheet, column, count) => {
                model.insert_columns(*sheet, *column, *count)
            }
            Edit::DeleteColumns(sheet, column, count) => {
                model.delete_columns(*sheet, *column, *count)
            }
            Edit::RenameSheet(sheet, name) => model.rename_sheet(*sheet, name),
            Edit::NewSheet => model.new_sheet(),
            Edit::DefinedName(name, formula) => model.new_defined_name(name, None, formula),
            Edit::HideRows(sheet, first, last, hidden) => {
                model.set_rows_hidden(*sheet, *first, *last, *hidden)
            }
        }
    }
}

/// The two models, kept in step.
pub struct Lockstep<'a> {
    incremental: UserModel<'a>,
    reference: UserModel<'a>,
    steps: Vec<Edit>,
    refused: usize,
}

impl<'a> Lockstep<'a> {
    /// Takes two copies of the same workbook, turns the incremental evaluator
    /// on in the first, gives both the evaluation depth `depth` if there is
    /// one, and evaluates both.
    pub fn new(
        mut incremental: Model<'a>,
        mut reference: Model<'a>,
        depth: Option<usize>,
    ) -> Result<Lockstep<'a>, String> {
        incremental.set_incremental(true);
        if let Some(depth) = depth {
            incremental.set_max_evaluation_depth(depth);
            reference.set_max_evaluation_depth(depth);
        }
        incremental.evaluate();
        reference.evaluate();
        let lockstep = Lockstep {
            incremental: UserModel::from_model(incremental),
            reference: UserModel::from_model(reference),
            steps: Vec::new(),
            refused: 0,
        };
        lockstep.compare("the first evaluation")?;
        Ok(lockstep)
    }

    pub fn reference(&self) -> &Model<'a> {
        &self.reference.model
    }

    /// Makes the edit in both models, which must both take it or both refuse
    /// it in the same words, and leave the same snapshot.
    pub fn apply(&mut self, edit: &Edit) -> Result<(), String> {
        self.steps.push(edit.clone());
        let incremental = edit.apply(&mut self.incremental);
        let reference = edit.apply(&mut self.reference);
        if incremental != reference {
            return Err(self.report(&format!(
                "the models answered differently: incremental {incremental:?}, reference {reference:?}"
            )));
        }
        if reference.is_err() {
            self.refused += 1;
        }
        self.compare("the edit")
    }

    /// The edits made so far, refused ones included.
    pub fn steps(&self) -> &[Edit] {
        &self.steps
    }

    /// How many of the edits both models refused.
    pub fn refused(&self) -> usize {
        self.refused
    }

    fn compare(&self, after: &str) -> Result<(), String> {
        let incremental = snapshot(&self.incremental.model);
        let reference = snapshot(&self.reference.model);
        if incremental == reference {
            return Ok(());
        }
        let mut differences = String::new();
        let longest = incremental.len().max(reference.len());
        let differing = (0..longest).filter(|&i| incremental.get(i) != reference.get(i));
        for i in differing.take(10) {
            let _ = write!(
                differences,
                "\n  incremental: {}\n  reference:   {}",
                incremental.get(i).map_or("(none)", String::as_str),
                reference.get(i).map_or("(none)", String::as_str),
            );
        }
        Err(self.report(&format!("the snapshots differ after {after}:{differences}")))
    }

    fn report(&self, problem: &str) -> String {
        let steps: Vec<String> = self.steps.iter().map(|s| format!("  {s:?}")).collect();
        format!(
            "{problem}\nafter {} steps:\n{}",
            self.steps.len(),
            steps.join("\n")
        )
    }
}

/// xorshift64, so that a seed names the same run everywhere.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// In `0..n`; `n` must not be 0.
    pub fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n
    }

    fn between(&mut self, first: i32, last: i32) -> i32 {
        first + self.below((last - first + 1) as u64) as i32
    }
}

/// Where the random edits of a run land: around the used area of a sheet.
struct Around<'m> {
    model: &'m Model<'m>,
    sheet: u32,
    rows: (i32, i32),
    columns: (i32, i32),
}

impl Around<'_> {
    fn cell(&self, rng: &mut Rng) -> (i32, i32) {
        (
            rng.between(self.rows.0, self.rows.1),
            rng.between(self.columns.0, self.columns.1),
        )
    }

    /// The name of a cell, sometimes qualified with a random sheet.
    fn name(&self, rng: &mut Rng, (row, column): (i32, i32)) -> String {
        let name = format!("{}{row}", number_to_column(column).unwrap_or_default());
        let sheets = &self.model.workbook.worksheets;
        if sheets.len() > 1 && rng.below(5) == 0 {
            let other = rng.below(sheets.len() as u64) as usize;
            format!("'{}'!{name}", sheets[other].name)
        } else {
            name
        }
    }

    fn random_name(&self, rng: &mut Rng) -> String {
        let cell = self.cell(rng);
        self.name(rng, cell)
    }

    fn formulas(&self) -> Vec<String> {
        self.model.workbook.worksheets[self.sheet as usize]
            .sheet_data
            .cells()
            .filter(|(_, _, cell)| {
                matches!(**cell, Cell::CellFormula { .. } | Cell::ArrayFormula { .. })
            })
            .filter_map(|(row, column, _)| {
                self.model
                    .get_cell_formula(self.sheet, row, column)
                    .ok()
                    .flatten()
            })
            .collect()
    }
}

/// The next edits of a random run over a workbook: one, or two for a cycle.
/// They are made around the used area of a random sheet of `model`, and some
/// copy the sheet's own formulas elsewhere. `made` counts the edits so far
/// and makes new names unique.
pub fn random_edits(model: &Model, rng: &mut Rng, made: usize) -> Vec<Edit> {
    let sheet = rng.below(model.workbook.worksheets.len() as u64) as u32;
    let worksheet = &model.workbook.worksheets[sheet as usize];
    let used = worksheet.dimension();
    // A margin around the used area, but not so far that edits miss it.
    let around = Around {
        model,
        sheet,
        rows: (used.min_row, (used.max_row + 3).min(used.min_row + 60)),
        columns: (
            used.min_column,
            (used.max_column + 3).min(used.min_column + 30),
        ),
    };
    let (row, column) = around.cell(rng);
    let input = |text: String| vec![Edit::Input(sheet, row, column, text)];
    match rng.below(20) {
        0 | 1 => input(format!("{}", rng.below(100))),
        2 => input(format!("text{}", rng.below(5))),
        3 => input("TRUE".to_string()),
        4 | 5 => {
            let a = around.random_name(rng);
            let b = around.random_name(rng);
            input(match rng.below(3) {
                0 => format!("={a}+1"),
                1 => format!("={a}*{b}"),
                _ => format!("=SUM({a},{b})"),
            })
        }
        6..=8 => {
            let formulas = around.formulas();
            match formulas.len() {
                0 => input("=1+1".to_string()),
                n => input(formulas[rng.below(n as u64) as usize].clone()),
            }
        }
        9 => input(String::new()),
        10 => vec![Edit::ClearRange(
            sheet,
            row,
            column,
            1 + rng.below(3) as i32,
            1 + rng.below(3) as i32,
        )],
        11 => {
            let (width, height) = (1 + rng.below(2) as i32, 1 + rng.below(2) as i32);
            let source = around.random_name(rng);
            vec![Edit::Cse(
                sheet,
                row,
                column,
                width,
                height,
                format!("={source}*2"),
            )]
        }
        12 => input(match rng.below(3) {
            0 => format!("=SEQUENCE({})", 1 + rng.below(4)),
            1 => {
                let (r, c) = around.cell(rng);
                let to = format!("{}{}", number_to_column(c).unwrap_or_default(), r + 2);
                format!("={}:{to}", around.name(rng, (r, c)))
            }
            _ => format!("=SEQUENCE(1,{})", 1 + rng.below(3)),
        }),
        13 => {
            let other = around.cell(rng);
            let here = around.name(rng, (row, column));
            let there = around.name(rng, other);
            vec![
                Edit::Input(sheet, row, column, format!("={there}+1")),
                Edit::Input(sheet, other.0, other.1, format!("={here}+1")),
            ]
        }
        14 => vec![Edit::Undo],
        15 => vec![Edit::Redo],
        16 => {
            let count = 1 + rng.below(2) as i32;
            vec![match rng.below(4) {
                0 => Edit::InsertRows(sheet, row, count),
                1 => Edit::DeleteRows(sheet, row, count),
                2 => Edit::InsertColumns(sheet, column, count),
                _ => Edit::DeleteColumns(sheet, column, count),
            }]
        }
        17 => vec![Edit::DefinedName(
            format!("lockstep{made}"),
            format!(
                "='{}'!${}${row}",
                worksheet.name,
                number_to_column(column).unwrap_or_default()
            ),
        )],
        18 => vec![Edit::HideRows(
            sheet,
            row,
            row + rng.below(2) as i32,
            rng.below(2) == 0,
        )],
        _ => vec![if rng.below(4) == 0 {
            Edit::NewSheet
        } else {
            Edit::RenameSheet(sheet, format!("Renamed{made}"))
        }],
    }
}

/// A random run of `steps` edits over two copies of a workbook.
pub fn run<'a>(
    incremental: Model<'a>,
    reference: Model<'a>,
    seed: u64,
    steps: usize,
    depth: Option<usize>,
) -> Result<Lockstep<'a>, String> {
    let mut lockstep = Lockstep::new(incremental, reference, depth)?;
    let mut rng = Rng::new(seed);
    while lockstep.steps.len() < steps {
        let made = lockstep.steps.len();
        for edit in random_edits(lockstep.reference(), &mut rng, made) {
            lockstep.apply(&edit)?;
        }
    }
    Ok(lockstep)
}

/// Computes `value(P)` of every formula through the incremental read path,
/// which panics in a debug build when a compute reads the sheet without a
/// node. The incremental evaluator must be on.
pub fn compute_every_formula(model: &mut Model) {
    let mut formulas = Vec::new();
    for (sheet, worksheet) in model.workbook.worksheets.iter().enumerate() {
        for (row, column, cell) in worksheet.sheet_data.cells() {
            if matches!(*cell, Cell::CellFormula { .. } | Cell::ArrayFormula { .. }) {
                formulas.push(CellReferenceIndex {
                    sheet: sheet as u32,
                    row,
                    column,
                });
            }
        }
    }
    for p in formulas {
        model.incr_value(p);
    }
}
