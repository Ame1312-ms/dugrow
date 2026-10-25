mod size;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use rusqlite::Connection;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use size::{fmt_delta, fmt_size, parse_human_size};

#[derive(Parser)]
#[command(
    name = "dugrow",
    version,
    about = "Track disk usage growth: which directories GREW, not just which are big"
)]
struct Cli {
    /// Path to the snapshot database
    /// (default: ~/.local/share/dugrow/dugrow.db)
    #[arg(long, global = true)]
    db: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Take a snapshot of one or more directory trees
    Scan {
        /// Root paths to scan
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        /// Only record directories at least this big (e.g. 0, 500KB, 1MiB, 2.5GB)
        #[arg(long, default_value = "0")]
        min_size: String,
    },
    /// Show which directories grew the most between snapshots
    Growth {
        /// Compare the latest snapshot against the newest one at least N days old
        #[arg(long)]
        days: Option<u32>,
        /// Compare the two most recent snapshots (this is the default)
        #[arg(long)]
        last: bool,
        /// How many directories to show
        #[arg(long, default_value_t = 20)]
        top: usize,
    },
    /// Show the biggest directories in the latest snapshot
    Top {
        /// How many directories to show
        #[arg(long, default_value_t = 20)]
        top: usize,
    },
    /// List stored snapshots
    Snapshots,
}

struct Snap {
    id: i64,
    taken_at: i64,
    root: String,
}

fn default_db_path() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".local").join("share").join("dugrow").join("dugrow.db")
}

fn open_db(cli_db: &Option<PathBuf>) -> Result<Connection> {
    let path = cli_db.clone().unwrap_or_else(default_db_path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("cannot create directory {}", parent.display()))?;
    }
    let conn = Connection::open(&path)
        .with_context(|| format!("cannot open database {}", path.display()))?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS snapshots (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             taken_at INTEGER NOT NULL,
             root TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS entries (
             snapshot_id INTEGER NOT NULL REFERENCES snapshots(id),
             dir_path TEXT NOT NULL,
             size_bytes INTEGER NOT NULL
         );
         CREATE INDEX IF NOT EXISTS idx_entries_snapshot ON entries(snapshot_id);",
    )?;
    Ok(conn)
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Walk one tree and return (dir_path, cumulative_size) for every directory.
/// Symlinks are never followed; unreadable entries are skipped with a warning.
fn scan_tree(root: &Path) -> Vec<(String, u64)> {
    let mut own: HashMap<String, u64> = HashMap::new();
    let mut dirs: Vec<String> = Vec::new();

    for entry in jwalk::WalkDir::new(root).follow_links(false) {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                eprintln!("warning: skipping unreadable entry: {e}");
                continue;
            }
        };
        let ft = entry.file_type();
        if ft.is_symlink() {
            continue;
        }
        let path = entry.path();
        if ft.is_dir() {
            dirs.push(path.to_string_lossy().into_owned());
        } else if ft.is_file() {
            let len = entry.metadata().map(|m| m.len()).unwrap_or(0);
            // A file's size belongs to its parent directory. A file given
            // directly as the scan root has no in-tree parent; record it
            // under its own path instead.
            let key = if path == root {
                if !dirs.iter().any(|d| d == &path.to_string_lossy().into_owned()) {
                    dirs.push(path.to_string_lossy().into_owned());
                }
                path.to_string_lossy().into_owned()
            } else {
                match path.parent() {
                    Some(p) => p.to_string_lossy().into_owned(),
                    None => path.to_string_lossy().into_owned(),
                }
            };
            *own.entry(key).or_insert(0) += len;
        }
    }

    // Cumulative sizes: start from each dir's own bytes, then add every
    // directory's total into its parent, deepest first.
    let mut total: HashMap<String, u64> = HashMap::new();
    for d in &dirs {
        total.insert(d.clone(), *own.get(d).unwrap_or(&0));
    }
    let mut by_depth: Vec<&String> = dirs.iter().collect();
    by_depth.sort_by_key(|d| Path::new(d).components().count());
    for d in by_depth.into_iter().rev() {
        if let Some(parent) = Path::new(d).parent() {
            let p = parent.to_string_lossy().into_owned();
            if let Some(pv) = total.get(&p).copied() {
                let dv = total[d];
                total.insert(p, pv + dv);
            }
        }
    }

    dirs.into_iter()
        .map(|d| {
            let s = total.get(&d).copied().unwrap_or(0);
            (d, s)
        })
        .collect()
}

fn cmd_scan(conn: &mut Connection, paths: &[PathBuf], min_size_s: &str) -> Result<()> {
    let min_size = parse_human_size(min_size_s)
        .map_err(|e| anyhow::anyhow!("--min-size: {e}"))?;

    for path in paths {
        let started = Instant::now();
        let root_abs = std::path::absolute(path)
            .with_context(|| format!("cannot resolve path {}", path.display()))?;
        if !root_abs.exists() {
            eprintln!("warning: {} does not exist, skipping", root_abs.display());
            continue;
        }
        let root_str = root_abs.to_string_lossy().into_owned();
        let entries = scan_tree(&root_abs);
        let root_total = entries
            .iter()
            .find(|(p, _)| p == &root_str)
            .map(|(_, s)| *s)
            .unwrap_or(0);
        let recorded: Vec<&(String, u64)> =
            entries.iter().filter(|(_, s)| *s >= min_size).collect();

        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO snapshots (taken_at, root) VALUES (?1, ?2)",
            rusqlite::params![now_unix(), root_str],
        )?;
        let snap_id = tx.last_insert_rowid();
        {
            let mut stmt = tx.prepare(
                "INSERT INTO entries (snapshot_id, dir_path, size_bytes) VALUES (?1, ?2, ?3)",
            )?;
            for (dir, s) in &recorded {
                stmt.execute(rusqlite::params![snap_id, dir, *s as i64])?;
            }
        }
        tx.commit()?;

        println!(
            "scan: {} dirs recorded, {} total, snapshot #{} ({:.1}s) [{}]",
            recorded.len(),
            fmt_size(root_total),
            snap_id,
            started.elapsed().as_secs_f64(),
            root_str
        );
    }
    Ok(())
}

fn list_snapshots(conn: &Connection) -> Result<Vec<Snap>> {
    let mut stmt =
        conn.prepare("SELECT id, taken_at, root FROM snapshots ORDER BY taken_at, id")?;
    let rows = stmt.query_map([], |r| {
        Ok(Snap {
            id: r.get(0)?,
            taken_at: r.get(1)?,
            root: r.get(2)?,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

fn entries_map(conn: &Connection, snapshot_id: i64) -> Result<HashMap<String, i64>> {
    let mut stmt =
        conn.prepare("SELECT dir_path, size_bytes FROM entries WHERE snapshot_id = ?1")?;
    let rows = stmt.query_map(rusqlite::params![snapshot_id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
    })?;
    let mut map = HashMap::new();
    for r in rows {
        let (p, s) = r?;
        map.insert(p, s);
    }
    Ok(map)
}

/// Pure delta computation: returns (path, delta, current_size) for every path
/// present in either snapshot. Positive delta = grew.
fn compute_deltas(
    old: &HashMap<String, i64>,
    new: &HashMap<String, i64>,
) -> Vec<(String, i64, i64)> {
    let mut out = Vec::with_capacity(old.len() + new.len());
    for (path, new_size) in new {
        let old_size = old.get(path).copied().unwrap_or(0);
        out.push((path.clone(), new_size - old_size, *new_size));
    }
    for (path, old_size) in old {
        if !new.contains_key(path) {
            out.push((path.clone(), -*old_size, 0));
        }
    }
    out
}

fn cmd_growth(conn: &Connection, days: Option<u32>, top_n: usize) -> Result<()> {
    let snaps = list_snapshots(conn)?;

    // Group snapshots by root, keeping chronological order.
    let mut roots: Vec<String> = Vec::new();
    let mut by_root: HashMap<String, Vec<&Snap>> = HashMap::new();
    for s in &snaps {
        if !by_root.contains_key(&s.root) {
            roots.push(s.root.clone());
        }
        by_root.entry(s.root.clone()).or_default().push(s);
    }

    let mut notes: Vec<String> = Vec::new();
    let mut rows: Vec<(String, i64, i64)> = Vec::new();
    let mut compared_any = false;

    for root in &roots {
        let list = &by_root[root];
        if list.len() < 2 {
            continue;
        }
        compared_any = true;
        let new_snap = list[list.len() - 1];
        let old_snap = match days {
            Some(d) => {
                let cutoff = new_snap.taken_at - i64::from(d) * 86_400;
                match list[..list.len() - 1]
                    .iter()
                    .rev()
                    .find(|s| s.taken_at <= cutoff)
                {
                    Some(s) => *s,
                    None => {
                        notes.push(format!(
                            "note: [{root}] no snapshot at least {d} day(s) old; \
                             comparing against the oldest available (#{})",
                            list[0].id
                        ));
                        list[0]
                    }
                }
            }
            None => list[list.len() - 2],
        };
        let old_map = entries_map(conn, old_snap.id)?;
        let new_map = entries_map(conn, new_snap.id)?;
        rows.extend(compute_deltas(&old_map, &new_map));
    }

    if !compared_any {
        println!(
            "dugrow: need at least two snapshots of the same root — \
             run `dugrow scan <path>` at least twice (over time) first."
        );
        return Ok(());
    }
    for n in &notes {
        println!("{n}");
    }

    rows.retain(|(_, delta, _)| *delta > 0);
    rows.sort_by(|a, b| b.1.cmp(&a.1));
    if rows.is_empty() {
        println!("no directory grew between the compared snapshots.");
        return Ok(());
    }
    println!("{:>12}  {:>12}  PATH", "GREW", "SIZE NOW");
    for (path, delta, cur) in rows.iter().take(top_n) {
        println!(
            "{:>12}  {:>12}  {}",
            fmt_delta(*delta),
            fmt_size(*cur as u64),
            path
        );
    }
    Ok(())
}

fn cmd_top(conn: &Connection, top_n: usize) -> Result<()> {
    let snaps = list_snapshots(conn)?;
    if snaps.is_empty() {
        println!("dugrow: no snapshots yet — run `dugrow scan <path>` first.");
        return Ok(());
    }
    // Latest snapshot per root.
    let mut latest: HashMap<String, i64> = HashMap::new();
    for s in &snaps {
        latest.insert(s.root.clone(), s.id);
    }
    let mut rows: Vec<(i64, String)> = Vec::new();
    for id in latest.values() {
        for (path, size) in entries_map(conn, *id)? {
            rows.push((size, path));
        }
    }
    rows.sort_by(|a, b| b.0.cmp(&a.0));
    println!("{:>12}  PATH", "SIZE");
    for (size, path) in rows.iter().take(top_n) {
        println!("{:>12}  {}", fmt_size(*size as u64), path);
    }
    Ok(())
}

fn fmt_time(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|dt| {
            dt.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|| ts.to_string())
}

fn cmd_snapshots(conn: &Connection) -> Result<()> {
    let mut stmt = conn.prepare(
        "SELECT s.id, s.taken_at, s.root, COUNT(e.dir_path)
         FROM snapshots s LEFT JOIN entries e ON e.snapshot_id = s.id
         GROUP BY s.id ORDER BY s.taken_at, s.id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, i64>(3)?,
        ))
    })?;
    let mut any = false;
    println!("{:>4}  {:<19}  {:>6}  ROOT", "ID", "TAKEN AT", "DIRS");
    for r in rows {
        let (id, taken_at, root, count) = r?;
        any = true;
        println!("{:>4}  {:<19}  {:>6}  {}", id, fmt_time(taken_at), count, root);
    }
    if !any {
        println!("(no snapshots yet)");
    }
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut conn = open_db(&cli.db)?;
    match &cli.command {
        Commands::Scan { paths, min_size } => cmd_scan(&mut conn, paths, min_size),
        Commands::Growth { days, top, .. } => cmd_growth(&conn, *days, *top),
        Commands::Top { top } => cmd_top(&conn, *top),
        Commands::Snapshots => cmd_snapshots(&conn),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deltas_growth_shrink_and_new() {
        let old: HashMap<String, i64> =
            [("/a".into(), 100), ("/b".into(), 50), ("/c".into(), 30)]
                .into_iter()
                .collect();
        let new: HashMap<String, i64> =
            [("/a".into(), 160), ("/b".into(), 50), ("/d".into(), 10)]
                .into_iter()
                .collect();
        let mut d = compute_deltas(&old, &new);
        d.sort();
        assert_eq!(
            d,
            vec![
                ("/a".to_string(), 60, 160),
                ("/b".to_string(), 0, 50),
                ("/c".to_string(), -30, 0),
                ("/d".to_string(), 10, 10),
            ]
        );
    }

    #[test]
    fn deltas_empty_old_means_all_new() {
        let old = HashMap::new();
        let new: HashMap<String, i64> = [("/x".into(), 7)].into_iter().collect();
        assert_eq!(compute_deltas(&old, &new), vec![("/x".to_string(), 7, 7)]);
    }
}
