#![allow(clippy::unwrap_used)]

// The static half of the read audit. A compute of the incremental evaluator
// must read the sheet through nodes, or the runtime does not know what its
// value depends on; the runtime half (`incr::ctx::audit_sheet_read`) panics
// on a direct read a test runs into. This half finds the ones no test runs:
// every direct read of the sheet in the code a formula's run goes through
// must be on the list below, with the reason it is sound.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// The code a formula's run can reach, relative to `src`.
const SCANNED: &[&str] = &[
    "functions",
    "expressions",
    "incr",
    "arithmetic.rs",
    "calc_result.rs",
    "cast.rs",
    "evaluation.rs",
    "implicit_intersection.rs",
    "model.rs",
    "units.rs",
    "volatile.rs",
];

/// What a direct read of the sheet looks like.
const READS: &[&str] = &[
    ".sheet_data",
    ".cell(",
    "fetch_cell(",
    "cell_mut",
    ".dimension()",
    "get_cell_formula(",
    "get_english_cell_formula(",
    ".get_style(",
    "is_empty_cell(",
    "get_cell_spill(",
    "get_cell_structure(",
    "column_cell_references(",
    "get_style_for_cell(",
    "get_cell_style_index(",
    "get_cell_style_or_none(",
    "get_cell_type(",
    "get_cell_value_by_index(",
    "get_formatted_cell_value(",
    "get_localized_cell_content(",
    "merged_cell_containing(",
];

const TRACKED: &str = "a note_*_read or SheetRead::tracked accounts for it";
const NOT_A_COMPUTE: &str = "no compute runs it: edits, the UI and the shipped engine";
const RECALC: &str = "the recalc between computes: it reads what the change log named";
const SHIPPED: &str = "the shipped engine's path; evaluate_cell returns before it while armed";
const DISCARDED: &str = "SheetRead::exempt: a compute drops what it reads (styles, clears)";
const SPILL_FALLBACK: &str =
    "SheetRead::exempt in incr_read_spill: what an earlier run left where no anchor writes now";
const SPILL_AREA: &str =
    "a dynamic spill's area: the tiles of each row; its merges: the metadata fingerprint";
const FORMULA_TEXT: &str = "ISFORMULA and FORMULATEXT note the content first; edits otherwise";
const PICKS_A_NODE: &str =
    "SheetRead::exempt: picks whether a read depends on value or content; value computes from content";
const PRIMITIVE: &str = "the read itself; each caller is on this list";

/// `(file, function, lines that read, why)`.
const ALLOWED: &[(&str, &str, usize, &str)] = &[
    ("evaluation.rs", "belongs_to_a_dynamic_array", 1, SHIPPED),
    ("evaluation.rs", "clip_to_used_area", 1, TRACKED),
    (
        "evaluation.rs",
        "dynamic_anchors_in_natural_order",
        1,
        SHIPPED,
    ),
    ("evaluation.rs", "dynamic_spills", 1, SHIPPED),
    ("evaluation.rs", "evaluate_cell", 1, SHIPPED),
    ("evaluation.rs", "evaluate_spill_cell", 1, SHIPPED),
    ("evaluation.rs", "formula_positions", 1, SHIPPED),
    ("evaluation.rs", "replay", 1, SHIPPED),
    ("evaluation.rs", "stored_value", 1, SPILL_FALLBACK),
    (
        "functions/financial/mod.rs",
        "get_array_of_numbers_generic",
        2,
        TRACKED,
    ),
    ("functions/information/mod.rs", "fn_isformula", 1, TRACKED),
    (
        "functions/lookup_and_reference/mod.rs",
        "fn_formulatext",
        1,
        TRACKED,
    ),
    ("functions/statistical/if_ifs.rs", "fn_countifs", 1, TRACKED),
    ("functions/subtotal.rs", "cell_is_subtotal", 1, TRACKED),
    ("incr/read.rs", "content_on_sheet", 2, TRACKED),
    ("incr/read.rs", "holds_computed_formula", 1, PICKS_A_NODE),
    ("incr/read.rs", "incr_circular", 1, TRACKED),
    ("incr/read.rs", "incr_dimension", 1, TRACKED),
    ("incr/recalc.rs", "a_row_changed", 1, RECALC),
    ("incr/recalc.rs", "incr_check", 3, RECALC),
    ("incr/recalc.rs", "incr_find_dynamic", 2, RECALC),
    ("incr/recalc.rs", "incr_rebuild", 3, RECALC),
    ("incr/recalc.rs", "incr_stores_own_circular", 1, RECALC),
    ("incr/recalc.rs", "incr_take_changes", 1, RECALC),
    ("incr/recalc.rs", "incr_write_back", 4, RECALC),
    ("incr/recalc.rs", "used_area_after", 2, RECALC),
    ("model.rs", "apply_result", 1, NOT_A_COMPUTE),
    ("model.rs", "can_clear_range", 1, NOT_A_COMPUTE),
    ("model.rs", "evaluate_node_in_context", 1, TRACKED),
    ("model.rs", "evaluate_range_of_numbers", 1, TRACKED),
    ("model.rs", "extend_to", 1, NOT_A_COMPUTE),
    ("model.rs", "fetch_cell", 3, PRIMITIVE),
    ("model.rs", "get_all_cells", 1, NOT_A_COMPUTE),
    ("model.rs", "get_cell_formula", 2, FORMULA_TEXT),
    ("model.rs", "get_cell_structure", 2, NOT_A_COMPUTE),
    ("model.rs", "get_cell_style_index", 3, NOT_A_COMPUTE),
    ("model.rs", "get_cell_style_or_none", 3, NOT_A_COMPUTE),
    ("model.rs", "get_cell_type", 2, NOT_A_COMPUTE),
    ("model.rs", "get_cell_value_by_index", 2, NOT_A_COMPUTE),
    ("model.rs", "get_column_style", 1, NOT_A_COMPUTE),
    ("model.rs", "get_english_cell_formula", 2, FORMULA_TEXT),
    ("model.rs", "get_formatted_cell_value", 3, NOT_A_COMPUTE),
    ("model.rs", "get_localized_cell_content", 4, NOT_A_COMPUTE),
    ("model.rs", "get_row_style", 1, NOT_A_COMPUTE),
    ("model.rs", "get_sheet_markup", 4, NOT_A_COMPUTE),
    ("model.rs", "get_style_for_cell", 3, NOT_A_COMPUTE),
    ("model.rs", "is_empty_cell", 2, NOT_A_COMPUTE),
    ("model.rs", "own_spill_cells_outside", 1, DISCARDED),
    ("model.rs", "plan_dynamic_spill", 2, SPILL_AREA),
    ("model.rs", "prepare_cell_for_user_input", 3, NOT_A_COMPUTE),
    ("model.rs", "range_clear_all", 2, NOT_A_COMPUTE),
    ("model.rs", "range_clear_contents", 1, NOT_A_COMPUTE),
    ("model.rs", "read_constants", 1, TRACKED),
    ("model.rs", "reset_dynamic_array_spills", 2, NOT_A_COMPUTE),
    ("model.rs", "set_user_array_formula", 4, NOT_A_COMPUTE),
    ("model.rs", "set_user_input", 3, NOT_A_COMPUTE),
    ("model.rs", "spill_style", 1, DISCARDED),
    ("model.rs", "get_cell_value_by_ref", 1, NOT_A_COMPUTE),
    ("model.rs", "update_cell_with_bool", 1, NOT_A_COMPUTE),
    ("model.rs", "update_cell_with_formula", 1, NOT_A_COMPUTE),
    ("model.rs", "update_cell_with_number", 1, NOT_A_COMPUTE),
    ("model.rs", "update_cell_with_text", 1, NOT_A_COMPUTE),
    ("units.rs", "compute_cell_units", 1, NOT_A_COMPUTE),
];

/// The lines of `source` that read the sheet, by the function they are in.
/// Comments are skipped, and so is a `#[cfg(test)]` module.
fn reads_by_function(source: &str) -> BTreeMap<String, usize> {
    let mut reads = BTreeMap::new();
    let mut function = String::from("(top level)");
    let mut lines = source.lines().peekable();
    while let Some(line) = lines.next() {
        let line = line.trim();
        if line == "#[cfg(test)]"
            && lines
                .peek()
                .is_some_and(|next| next.trim().starts_with("mod "))
        {
            break;
        }
        if line.starts_with("//") {
            continue;
        }
        if let Some(name) = defined_function(line) {
            function = name;
        }
        if READS.iter().any(|read| line.contains(read)) {
            *reads.entry(function.clone()).or_insert(0) += 1;
        }
    }
    reads
}

fn defined_function(line: &str) -> Option<String> {
    let start = if line.starts_with("fn ") {
        3
    } else {
        line.find(" fn ")? + 4
    };
    let rest = &line[start..];
    let end = rest.find(|c: char| !(c.is_alphanumeric() || c == '_'))?;
    Some(rest[..end].to_string())
}

fn rust_files(path: &Path, files: &mut Vec<std::path::PathBuf>) {
    if path.is_dir() {
        let mut entries: Vec<_> = fs::read_dir(path)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        entries.sort();
        for entry in entries {
            rust_files(&entry, files);
        }
    } else if path.extension().is_some_and(|e| e == "rs") {
        files.push(path.to_path_buf());
    }
}

#[test]
fn every_direct_read_of_the_sheet_is_allowed() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    for scanned in SCANNED {
        rust_files(&src.join(scanned), &mut files);
    }
    let mut found = BTreeMap::new();
    for file in &files {
        let name = file
            .strip_prefix(&src)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        for (function, count) in reads_by_function(&fs::read_to_string(file).unwrap()) {
            found.insert((name.clone(), function), count);
        }
    }
    let allowed: BTreeMap<_, _> = ALLOWED
        .iter()
        .map(|(file, function, count, _)| ((file.to_string(), function.to_string()), *count))
        .collect();
    let mut problems = Vec::new();
    for ((file, function), count) in &found {
        match allowed.get(&(file.clone(), function.clone())) {
            Some(allowed) if allowed == count => {}
            Some(allowed) => problems.push(format!(
                "{file} {function}: {count} direct reads, {allowed} allowed"
            )),
            None => problems.push(format!(
                "{file} {function}: {count} direct reads, none allowed"
            )),
        }
    }
    for (file, function) in allowed.keys() {
        if !found.contains_key(&(file.clone(), function.clone())) {
            problems.push(format!(
                "{file} {function}: allowed, but it reads nothing now"
            ));
        }
    }
    assert!(
        problems.is_empty(),
        "read the sheet through a node, or add the read to ALLOWED with its reason:\n  {}",
        problems.join("\n  ")
    );
}

#[test]
fn reads_are_counted_by_the_function_they_are_in() {
    let source = "\
fn a() {
    let x = worksheet.cell(1, 1);
    // worksheet.cell(2, 2)
}
pub(crate) fn b<T>(&self) {
    self.fetch_cell(p);
    let d = w.dimension();
}
#[cfg(test)]
mod tests {
    fn c() { worksheet.cell(1, 1); }
}
";
    let reads = reads_by_function(source);
    assert_eq!(
        reads.into_iter().collect::<Vec<_>>(),
        vec![("a".to_string(), 1), ("b".to_string(), 2)]
    );
}
