//! The `summary/` on-disk format: `runs.json` plus one index-aligned score
//! file per WPT directory. Shared with
//! https://github.com/DioxusLabs/blitz-wpt-results (see README.md).

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use serde::{Deserialize, Serialize};
use wptreport::AreaScores;

/// Per-area scores for a single run, stored as a compact
/// `[total_tests, total_score, total_subtests, total_subtests_passed]` array
/// (with `total_score` rounded to 1dp)
pub type ScoreTuple = (u32, f64, u32, u32);

pub fn score_tuple(scores: &AreaScores) -> ScoreTuple {
    (
        scores.tests.total,
        (scores.servo_score() * 10.0).round() / 10.0,
        scores.subtests.total,
        scores.subtests.pass,
    )
}

/// Metadata about a single WPT run, stored once in `runs.json` and shared by
/// all per-area score files (which are index-aligned with it)
#[derive(Clone, Serialize, Deserialize)]
pub struct RunMeta {
    /// When the run started, RFC3339 format
    pub date: String,
    /// The revision of the WPT test suite that was run (9-char sha)
    pub wpt_revision: String,
    /// The browser version that was tested (blitz-wpt-results stores the
    /// blitz commit sha here)
    pub product_revision: String,
    /// First line of the tested commit's message (unused for browsers)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit_message: Option<String>,
    /// The wpt.fyi run ID. Used to de-duplicate runs, since
    /// `product_revision` (a browser version) is not unique.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<u64>,
}

impl RunMeta {
    fn dedup_key(&self) -> String {
        match self.run_id {
            Some(id) => format!("run:{id}"),
            None => self.product_revision.clone(),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct RunsFile {
    runs: Vec<RunMeta>,
}

/// The pseudo-area holding the whole-run total (sum of every top-level
/// area), stored as `total.json` next to `runs.json` rather than under
/// `areas/` so it can't collide with a WPT directory
pub const TOTAL_AREA: &str = "";

/// Scores for a single area (one WPT folder). `scores` is index-aligned with
/// `runs.json`: one entry per run. `null` marks a run with no data for this
/// area.
#[derive(Serialize, Deserialize)]
pub struct AreaFile {
    pub scores: Vec<Option<ScoreTuple>>,
}

/// A run scored across every directory of the WPT tree
pub struct ScoredRun {
    pub meta: RunMeta,
    pub scores: BTreeMap<String, ScoreTuple>,
}

/// The whole summary dataset: shared run metadata plus one score file per
/// area (with [`TOTAL_AREA`] as the whole-run total)
#[derive(Default)]
pub struct SummaryStore {
    pub runs: Vec<RunMeta>,
    pub areas: BTreeMap<String, Vec<Option<ScoreTuple>>>,
}

fn area_path(dir: &Path, area: &str) -> std::path::PathBuf {
    if area == TOTAL_AREA {
        dir.join("total.json")
    } else {
        dir.join("areas").join(format!("{area}.json"))
    }
}

fn load_area_file(path: &Path, name: &str, run_count: usize) -> Vec<Option<ScoreTuple>> {
    let file: AreaFile = serde_json::from_slice(&std::fs::read(path).unwrap())
        .expect("area file should be valid JSON");
    assert_eq!(
        file.scores.len(),
        run_count,
        "area file {name} is misaligned with runs.json"
    );
    file.scores
}

fn load_area_files(
    dir: &Path,
    prefix: &str,
    run_count: usize,
    areas: &mut BTreeMap<String, Vec<Option<ScoreTuple>>>,
) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            let name = path.file_name().unwrap().to_str().unwrap();
            load_area_files(&path, &format!("{prefix}{name}/"), run_count, areas);
        } else if path.extension().is_some_and(|ext| ext == "json") {
            let stem = path.file_stem().unwrap().to_str().unwrap();
            let name = format!("{prefix}{stem}");
            let scores = load_area_file(&path, &name, run_count);
            areas.insert(name, scores);
        }
    }
}

impl SummaryStore {
    pub fn load(dir: &Path) -> Option<Self> {
        let runs_file: RunsFile =
            serde_json::from_slice(&std::fs::read(dir.join("runs.json")).ok()?)
                .expect("runs.json should be valid JSON");
        let mut areas = BTreeMap::new();
        load_area_files(&dir.join("areas"), "", runs_file.runs.len(), &mut areas);
        let total = area_path(dir, TOTAL_AREA);
        if total.exists() {
            let scores = load_area_file(&total, "total", runs_file.runs.len());
            areas.insert(TOTAL_AREA.to_string(), scores);
        }
        Some(SummaryStore {
            runs: runs_file.runs,
            areas,
        })
    }

    /// Append runs (de-duplicated by wpt.fyi run ID if set, otherwise by
    /// product revision), then re-sort all files consistently by
    /// (date, product_revision). A run that is already present is skipped,
    /// or has its scores replaced when `replace` is set.
    pub fn append(&mut self, new_runs: Vec<ScoredRun>, replace: bool) {
        let mut existing: HashMap<String, usize> = self
            .runs
            .iter()
            .enumerate()
            .map(|(idx, run)| (run.dedup_key(), idx))
            .collect();

        for run in new_runs {
            let key = run.meta.dedup_key();
            let idx = match existing.get(&key) {
                Some(_) if !replace => continue,
                Some(&idx) => idx,
                None => {
                    let idx = self.runs.len();
                    existing.insert(key, idx);
                    for scores in self.areas.values_mut() {
                        scores.push(None);
                    }
                    self.runs.push(run.meta.clone());
                    idx
                }
            };

            // Ensure every area of this run has a file, padding new areas
            // with nulls for pre-existing runs
            let run_count = self.runs.len();
            for area in run.scores.keys() {
                self.areas
                    .entry(area.clone())
                    .or_insert_with(|| vec![None; run_count]);
            }

            // Set this run's score in every area file (null for areas the
            // run has no data for)
            for (area, scores) in &mut self.areas {
                scores[idx] = run.scores.get(area).copied();
            }
            self.runs[idx] = run.meta;
        }

        self.sort();
    }

    /// Sort runs by (date, product_revision), applying the same permutation
    /// to every area file so they stay index-aligned
    fn sort(&mut self) {
        let mut order: Vec<usize> = (0..self.runs.len()).collect();
        order.sort_by(|&a, &b| {
            let ka = (&self.runs[a].date, &self.runs[a].product_revision);
            let kb = (&self.runs[b].date, &self.runs[b].product_revision);
            ka.cmp(&kb)
        });

        self.runs = order.iter().map(|&i| self.runs[i].clone()).collect();
        for scores in self.areas.values_mut() {
            *scores = order.iter().map(|&i| scores[i]).collect();
        }
    }

    /// Write the store to `dir` as runs.json + areas/<area>.json, with one
    /// line per run in each file so appends produce one-line git diffs
    pub fn write(&self, dir: &Path) {
        let areas_dir = dir.join("areas");
        std::fs::create_dir_all(&areas_dir).unwrap();

        write_json_lines(
            &dir.join("runs.json"),
            "{\n\"runs\":[\n",
            self.runs.iter().map(serde_json::to_string),
        );

        for (area, scores) in &self.areas {
            let path = area_path(dir, area);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            write_json_lines(
                &path,
                "{\n\"scores\":[\n",
                scores.iter().map(serde_json::to_string),
            );
        }
    }
}

fn write_json_lines(
    path: &Path,
    header: &str,
    lines: impl Iterator<Item = serde_json::Result<String>>,
) {
    let mut json = String::from(header);
    for (i, line) in lines.enumerate() {
        if i > 0 {
            json.push_str(",\n");
        }
        json.push_str(&line.unwrap());
    }
    json.push_str("\n]}\n");
    std::fs::write(path, json).unwrap();
}
