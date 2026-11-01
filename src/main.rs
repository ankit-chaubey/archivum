/*
 * Copyright 2026 Ankit Chaubey <ankitchaubey.dev@gmail.com>
 * github.com/ankit-chaubey
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

mod cat;
mod checksum;
mod completions;
mod compress;
mod config;
mod diff;
mod fsutil;
mod index;
mod merge;
mod output;
mod prune;
mod repair;
mod restore;
mod scan;
mod search;
mod sign;
mod stats;
mod tar_writer;
mod update;
mod utils;
mod verify;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use colored::Colorize;
use std::path::PathBuf;

use compress::CompressionAlgo;
use config::Config;
use output::OutputCtx;

/// Archivum - deterministic, split, checksummed, compressed archive system.
/// Also available as `arc` (same binary, both names).
#[derive(Parser)]
#[command(
    name = "archivum",
    version = env!("CARGO_PKG_VERSION"),
    author = "Ankit Chaubey <ankitchaubey.dev@gmail.com>",
    about = "Deterministic, split, checksummed, compressed archive system",
    after_help = concat!(
        "EXAMPLES:\n",
        "  archivum create ./photos ./backup\n",
        "  arc create ./photos ./backup --compress zstd --split-gb 2\n",
        "  archivum list ./backup/index.arc.json\n",
        "  archivum restore ./backup/index.arc.json ./restored\n",
        "  archivum verify ./backup/index.arc.json\n",
        "  archivum diff ./backup/index.arc.json ./photos\n",
        "  archivum update ./backup/index.arc.json ./photos ./backup2\n",
        "  archivum stats ./backup/index.arc.json\n",
        "  archivum search ./backup/index.arc.json '*.jpg'\n",
        "  archivum cat ./backup/index.arc.json photos/img.jpg > img.jpg\n",
        "  archivum completions bash >> ~/.bashrc\n",
        "  archivum setup\n",
        "\nCONFIG: ~/.config/archivum/config.toml (run `archivum setup` to configure)"
    )
)]
pub struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Output JSON instead of human-readable text
    #[arg(long, global = true)]
    json: bool,

    /// Suppress all output except errors
    #[arg(long, global = true, short = 'q')]
    quiet: bool,

    /// Show what would happen without doing it
    #[arg(long, global = true, short = 'n')]
    dry_run: bool,

    /// Append all output to this log file
    #[arg(long, global = true, value_name = "PATH")]
    log_file: Option<PathBuf>,

    /// Only accept indexes signed with this public key (made by `archivum keygen`)
    #[arg(long, global = true, value_name = "FILE")]
    pubkey: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Commands {
    /// Create a new archive from a source directory
    Create {
        #[arg(value_name = "SOURCE")]
        source: PathBuf,
        #[arg(value_name = "OUTPUT")]
        output: PathBuf,
        /// Max size per archive part in GB (config default: 4.0)
        #[arg(long, value_name = "GB")]
        split_gb: Option<f64>,
        /// Max files per archive part (0 = disabled)
        #[arg(long, value_name = "N", default_value = "0")]
        split_files: usize,
        /// Compression algorithm: none | gzip | bzip2 | lz4 | zstd
        #[arg(long, value_name = "ALGO")]
        compress: Option<String>,
        /// Zstd compression level (1-22)
        #[arg(long, value_name = "LEVEL")]
        zstd_level: Option<i32>,
        /// Exclude glob patterns (repeatable)
        #[arg(long, value_name = "PATTERN")]
        exclude: Vec<String>,
        /// Parallel checksum threads (config default: 4)
        #[arg(long, value_name = "N")]
        threads: Option<usize>,
        /// Deduplicate files with identical SHA-256
        #[arg(long)]
        dedup: bool,
        /// Optional description stored in the index
        #[arg(long, value_name = "TEXT")]
        notes: Option<String>,
        /// Zstd worker threads (0 = auto, all cores; config default: 0)
        #[arg(long, value_name = "N")]
        zstd_threads: Option<u32>,
        /// Replace an existing archive in the output directory
        #[arg(long)]
        overwrite: bool,
        /// Sign the index with this secret key (made by `archivum keygen`)
        #[arg(long, value_name = "KEY")]
        sign: Option<PathBuf>,
    },

    /// List contents and statistics of an archive
    List {
        #[arg(value_name = "INDEX")]
        index: PathBuf,
        /// Show all file entries (not just summary)
        #[arg(long, short)]
        verbose: bool,
        /// Filter entries by glob pattern
        #[arg(long, value_name = "PATTERN")]
        filter: Option<String>,
    },

    /// Restore an archive to a target directory
    Restore {
        #[arg(value_name = "INDEX")]
        index: PathBuf,
        #[arg(value_name = "TARGET")]
        target: PathBuf,
        /// Only restore files matching this glob
        #[arg(long, value_name = "PATTERN")]
        filter: Option<String>,
        /// Overwrite existing files
        #[arg(long, short)]
        force: bool,
        /// Restore Unix permissions
        #[arg(long)]
        restore_permissions: bool,
        /// Parallel workers (one tar part per worker; config default: 4)
        #[arg(long, value_name = "N")]
        threads: Option<usize>,
    },

    /// Verify archive integrity (checksums + structure)
    Verify {
        #[arg(value_name = "INDEX")]
        index: PathBuf,
        /// Keep scanning after the first problem (the exit code is non-zero either way)
        #[arg(long, short = 'c')]
        continue_on_error: bool,
        /// Parallel workers (one tar part per worker; config default: 4)
        #[arg(long, value_name = "N")]
        threads: Option<usize>,
    },

    /// Compare archive against source directory (drift detection)
    Diff {
        #[arg(value_name = "INDEX")]
        index: PathBuf,
        #[arg(value_name = "SOURCE")]
        source: PathBuf,
        /// Show only changed files
        #[arg(long)]
        changed_only: bool,
        /// Use SHA-256 to detect changes (not just mtime+size)
        #[arg(long)]
        checksum: bool,
        /// Exclude glob patterns (repeatable; config excludes always apply)
        #[arg(long, value_name = "PATTERN")]
        exclude: Vec<String>,
    },

    /// Print detailed info about a specific file in the archive
    Info {
        #[arg(value_name = "INDEX")]
        index: PathBuf,
        #[arg(value_name = "FILE")]
        file: PathBuf,
    },

    /// Extract a single file from the archive to a path
    Extract {
        #[arg(value_name = "INDEX")]
        index: PathBuf,
        #[arg(value_name = "FILE")]
        file: PathBuf,
        #[arg(long, value_name = "OUTPUT")]
        output: Option<PathBuf>,
        /// Overwrite the output file if it exists
        #[arg(long, short)]
        force: bool,
    },

    /// Stream a single file from the archive to stdout
    Cat {
        #[arg(value_name = "INDEX")]
        index: PathBuf,
        #[arg(value_name = "FILE")]
        file: PathBuf,
    },

    /// Search the archive index by glob or substring
    Search {
        #[arg(value_name = "INDEX")]
        index: PathBuf,
        /// Glob or substring pattern
        #[arg(value_name = "PATTERN")]
        pattern: String,
    },

    /// Show detailed statistics for an archive
    Stats {
        #[arg(value_name = "INDEX")]
        index: PathBuf,
    },

    /// Incremental update: re-archive only changed/new files
    Update {
        /// Existing archive index to update from
        #[arg(value_name = "OLD_INDEX")]
        old_index: PathBuf,
        /// Source directory to archive
        #[arg(value_name = "SOURCE")]
        source: PathBuf,
        /// Output directory for new delta parts
        #[arg(value_name = "OUTPUT")]
        output: PathBuf,
        #[arg(long, value_name = "GB")]
        split_gb: Option<f64>,
        #[arg(long, value_name = "N", default_value = "0")]
        split_files: usize,
        #[arg(long, value_name = "ALGO")]
        compress: Option<String>,
        #[arg(long, value_name = "LEVEL")]
        zstd_level: Option<i32>,
        #[arg(long, value_name = "PATTERN")]
        exclude: Vec<String>,
        #[arg(long, value_name = "N")]
        threads: Option<usize>,
        /// Use SHA-256 comparison to detect changes
        #[arg(long)]
        checksum: bool,
        /// Zstd worker threads (0 = auto, all cores)
        #[arg(long, value_name = "N")]
        zstd_threads: Option<u32>,
        /// Deduplicate identical files within the new data
        #[arg(long)]
        dedup: bool,
        /// Sign the new index with this secret key
        #[arg(long, value_name = "KEY")]
        sign: Option<PathBuf>,
    },

    /// Prune old archives in a directory
    Prune {
        /// Directory containing archive subdirectories
        #[arg(value_name = "DIR")]
        dir: PathBuf,
        /// Always keep at least N archives (config default: 3)
        #[arg(long, value_name = "N")]
        keep: Option<usize>,
        /// Delete archives older than N days (0 = any age; config default: 30)
        #[arg(long, value_name = "DAYS")]
        max_age: Option<u64>,
    },

    /// Merge multiple archives into one
    Merge {
        /// Two or more index.arc.json paths to merge
        #[arg(value_name = "INDEX", num_args = 2..)]
        indexes: Vec<PathBuf>,
        /// Output directory for merged archive
        #[arg(long, value_name = "OUTPUT", required = true)]
        output: PathBuf,
        #[arg(long, value_name = "GB")]
        split_gb: Option<f64>,
        #[arg(long, value_name = "ALGO")]
        compress: Option<String>,
        #[arg(long, value_name = "LEVEL")]
        zstd_level: Option<i32>,
        /// Zstd worker threads (0 = auto, all cores)
        #[arg(long, value_name = "N")]
        zstd_threads: Option<u32>,
        /// Replace an existing archive in the output directory
        #[arg(long)]
        overwrite: bool,
        /// Sign the merged index with this secret key
        #[arg(long, value_name = "KEY")]
        sign: Option<PathBuf>,
    },

    /// Generate an Ed25519 key pair for signing indexes (writes NAME.key and NAME.pub)
    Keygen {
        #[arg(value_name = "NAME")]
        name: PathBuf,
    },

    /// Rebuild a missing index.arc.json from existing tar parts
    Repair {
        /// Directory containing the archive parts
        #[arg(value_name = "DIR")]
        dir: PathBuf,
        /// Compression algorithm of the parts (auto = detect from the files)
        #[arg(long, value_name = "ALGO", default_value = "auto")]
        compression: String,
    },

    /// Generate shell completion scripts
    Completions {
        /// Shell to generate for: bash | zsh | fish | powershell | elvish
        #[arg(value_name = "SHELL")]
        shell: String,
    },

    /// Interactive configuration setup
    Setup,

    /// Print current configuration
    Config,
}

fn main() {
    if let Err(e) = run() {
        eprintln!("{} {}", "error:".red().bold(), e);
        for cause in e.chain().skip(1) {
            eprintln!("  {} {}", "caused by:".yellow(), cause);
        }
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let cfg = Config::load();

    let out = OutputCtx::new(
        cli.json || cfg.output.json,
        cli.quiet || cfg.output.quiet,
        cli.dry_run,
        cli.log_file.as_deref(),
    )?;

    let pubkey_path = cli
        .pubkey
        .clone()
        .or_else(|| (!cfg.security.pubkey.is_empty()).then(|| PathBuf::from(&cfg.security.pubkey)));
    if let Some(p) = pubkey_path {
        sign::set_trusted(sign::load_public_key(&p)?);
    }
    let load_signer = |flag: &Option<PathBuf>| -> Result<Option<ed25519_dalek::SigningKey>> {
        let path = flag.clone().or_else(|| {
            (!cfg.security.sign_key.is_empty()).then(|| PathBuf::from(&cfg.security.sign_key))
        });
        path.map(|p| sign::load_signing_key(&p)).transpose()
    };

    match cli.command {
        Commands::Create {
            source,
            output,
            split_gb,
            split_files,
            compress,
            zstd_level,
            mut exclude,
            threads,
            dedup,
            notes,
            zstd_threads,
            overwrite,
            sign: sign_flag,
        } => {
            utils::print_banner(&out);
            let signer = load_signer(&sign_flag)?;

            let compress_str = compress.as_deref().unwrap_or(&cfg.defaults.compress);
            let algo = CompressionAlgo::parse(compress_str)
                .with_context(|| format!("Unknown compression algorithm: '{compress_str}'"))?;
            let zstd_lvl = zstd_level.unwrap_or(cfg.defaults.zstd_level);
            let split =
                (split_gb.unwrap_or(cfg.defaults.split_gb) * 1024.0 * 1024.0 * 1024.0) as u64;
            let split_f = if split_files > 0 {
                split_files
            } else {
                cfg.defaults.split_files
            };
            let thread_count = threads.unwrap_or(cfg.defaults.threads);
            let do_dedup = dedup || cfg.create.dedup;
            let zstd_thr = zstd_threads.unwrap_or(cfg.defaults.zstd_threads);

            let mut all_excludes = cfg.create.exclude.clone();
            all_excludes.append(&mut exclude);

            out.println(&format!(
                "{} {} -> {}",
                "Creating archive:".cyan().bold(),
                source.display().to_string().yellow(),
                output.display().to_string().yellow()
            ));
            out.println(&format!(
                "  split={:.1} GB  split-files={}  compress={}  zstd-level={}  dedup={}  threads={}",
                split_gb.unwrap_or(cfg.defaults.split_gb),
                split_f,
                algo.name().green(),
                zstd_lvl,
                do_dedup,
                thread_count
            ));
            out.println("");

            if out.dry_run {
                out.dry(&format!("would scan: {}", source.display()));
                out.dry(&format!("would create: {}", output.display()));
                return Ok(());
            }

            if !source.is_dir() {
                anyhow::bail!("Source directory does not exist: {}", source.display());
            }
            if fsutil::same_path(&source, &output) {
                anyhow::bail!("The output directory must differ from the source directory");
            }
            fsutil::ensure_clean_output(&output, overwrite)?;
            std::fs::create_dir_all(&output)
                .with_context(|| format!("Failed to create output dir {}", output.display()))?;
            // never archive the archive itself when the output lives inside the source
            let skip = vec![output.canonicalize()?];

            let scan = scan::scan_directory_with(&source, &all_excludes, &skip)
                .with_context(|| format!("Failed to scan {}", source.display()))?;
            for w in &scan.warnings {
                out.eprintln(&format!("  {} {}", "warning:".yellow(), w));
            }
            let skipped = scan.warnings.len();

            let mut idx = index::ArchivumIndex::build(scan.entries, zstd_lvl);

            if let Some(n) = notes {
                idx.header.notes = n;
            } else if !cfg.create.notes.is_empty() {
                idx.header.notes = cfg.create.notes.clone();
            }

            checksum::compute_checksums(&source, &mut idx, thread_count, do_dedup)?;

            tar_writer::write_archive(
                &source,
                &mut idx,
                &tar_writer::WriteOpts {
                    out_dir: output.clone(),
                    algo: algo.clone(),
                    zstd_level: zstd_lvl,
                    zstd_threads: zstd_thr,
                    split_bytes: split,
                    split_files: split_f,
                    workers: thread_count,
                },
            )?;

            let index_path = output.join(fsutil::INDEX_NAME);
            idx.write_signed(&index_path, signer.as_ref())?;

            if skipped > 0 {
                out.eprintln(&format!(
                    "  {} {} path(s) could not be read and are NOT in the archive",
                    "warning:".yellow().bold(),
                    skipped
                ));
            }

            let deduped = idx.entries.iter().filter(|e| e.dedup_of.is_some()).count();

            out.println("");
            out.println(&"─".repeat(60).dimmed().to_string());
            out.println(&"  Archive created successfully!".green().bold().to_string());
            out.println(&format!(
                "  Files : {}  |  Dirs : {}  |  Parts : {}{}",
                idx.header.total_files.to_string().yellow(),
                idx.header.total_dirs.to_string().yellow(),
                idx.header.parts.len().to_string().yellow(),
                if deduped > 0 {
                    format!("  |  Deduped: {}", deduped.to_string().cyan())
                } else {
                    String::new()
                }
            ));
            out.println(&format!(
                "  Size  : {}  |  Compression: {}",
                utils::human(idx.header.total_size).cyan(),
                algo.name().green()
            ));
            out.println(&format!(
                "  Index : {}",
                index_path.display().to_string().cyan()
            ));
            out.println(&"─".repeat(60).dimmed().to_string());
        }

        Commands::List {
            index,
            verbose,
            filter,
        } => {
            let idx = index::ArchivumIndex::read(&index)
                .with_context(|| format!("Failed to read index: {}", index.display()))?;
            if out.json {
                idx.print_summary_json()?;
            } else {
                idx.print_summary(verbose, filter.as_deref(), &out)?;
            }
        }

        Commands::Restore {
            index,
            target,
            filter,
            force,
            restore_permissions,
            threads,
        } => {
            utils::print_banner(&out);
            let do_force = force || cfg.restore.force;
            let do_perm = restore_permissions || cfg.restore.restore_permissions;
            let workers = threads.unwrap_or(cfg.defaults.threads);
            restore::restore(
                &index,
                &target,
                filter.as_deref(),
                do_force,
                do_perm,
                workers,
                &out,
            )?;
        }

        Commands::Verify {
            index,
            continue_on_error,
            threads,
        } => {
            utils::print_banner(&out);
            let workers = threads.unwrap_or(cfg.defaults.threads);
            verify::verify(&index, continue_on_error, workers, &out)?;
        }

        Commands::Diff {
            index,
            source,
            changed_only,
            checksum,
            mut exclude,
        } => {
            let use_cs = checksum || cfg.update.checksum_diff;
            let mut all_excludes = cfg.create.exclude.clone();
            all_excludes.append(&mut exclude);
            diff::diff(&index, &source, changed_only, use_cs, &all_excludes, &out)?;
        }

        Commands::Info { index, file } => {
            let idx = index::ArchivumIndex::read(&index)?;
            if let Some(entry) = idx.entries.iter().find(|e| e.path == file) {
                if out.json {
                    let j = serde_json::json!({
                        "path": entry.path,
                        "type": format!("{:?}", entry.entry_type),
                        "size": entry.size,
                        "sha256": entry.sha256,
                        "loc": entry.loc,
                        "mtime": entry.mtime,
                        "unix_mode": entry.unix_mode,
                        "dedup_of": entry.dedup_of
                    });
                    println!("{}", serde_json::to_string_pretty(&j).unwrap());
                } else {
                    println!("{}", "─".repeat(50).dimmed());
                    println!("{} {}", "File:".cyan().bold(), entry.path.display());
                    println!(
                        "{} {}",
                        "Type:".cyan(),
                        format!("{:?}", entry.entry_type).green()
                    );
                    println!("{} {}", "Size:".cyan(), utils::human(entry.size).yellow());
                    println!(
                        "{} {}",
                        "SHA-256:".cyan(),
                        entry.sha256.as_deref().unwrap_or("-").yellow()
                    );
                    println!(
                        "{} {}",
                        "Location:".cyan(),
                        entry
                            .loc
                            .map(|l| format!("part{:03} @ {}+{}", l.part, l.off, l.len))
                            .unwrap_or_else(|| "-".into())
                            .yellow()
                    );
                    if let Some(m) = entry.mtime {
                        println!("{} {}", "Modified:".cyan(), utils::fmt_time(m).yellow());
                    }
                    if let Some(mode) = entry.unix_mode {
                        println!("{} {:o}", "Mode:".cyan(), mode);
                    }
                    if let Some(ref orig) = entry.dedup_of {
                        println!(
                            "{} {}",
                            "Dedup of:".cyan(),
                            orig.display().to_string().yellow()
                        );
                    }
                    println!("{}", "─".repeat(50).dimmed());
                }
            } else {
                anyhow::bail!("File not found in archive: {}", file.display());
            }
        }

        Commands::Extract {
            index,
            file,
            output,
            force,
        } => {
            let idx = index::ArchivumIndex::read(&index)?;
            let base = index.parent().unwrap_or(std::path::Path::new("."));
            restore::extract_single(&idx, base, &file, output.as_deref(), force, &out)?;
        }

        Commands::Cat { index, file } => {
            cat::cat(&index, &file)?;
        }

        Commands::Search { index, pattern } => {
            search::search(&index, &pattern, &out)?;
        }

        Commands::Stats { index } => {
            stats::stats(&index, &out)?;
        }

        Commands::Update {
            old_index,
            source,
            output,
            split_gb,
            split_files,
            compress,
            zstd_level,
            mut exclude,
            threads,
            checksum,
            zstd_threads,
            dedup,
            sign: sign_flag,
        } => {
            utils::print_banner(&out);
            let signer = load_signer(&sign_flag)?;
            // None = compression of the newest part of the archive being updated
            let algo = compress
                .as_deref()
                .map(CompressionAlgo::parse)
                .transpose()?;
            let zstd_lvl = zstd_level.unwrap_or(cfg.defaults.zstd_level);
            let split =
                (split_gb.unwrap_or(cfg.defaults.split_gb) * 1024.0 * 1024.0 * 1024.0) as u64;
            let split_f = if split_files > 0 {
                split_files
            } else {
                cfg.defaults.split_files
            };
            let thread_count = threads.unwrap_or(cfg.defaults.threads);
            let use_cs = checksum || cfg.update.checksum_diff;
            let mut all_excludes = cfg.create.exclude.clone();
            all_excludes.append(&mut exclude);

            update::update(
                update::UpdateParams {
                    old_index_path: &old_index,
                    source: &source,
                    output_dir: &output,
                    split_bytes: split,
                    split_files: split_f,
                    algo,
                    sign_key: signer.as_ref(),
                    zstd_level: zstd_lvl,
                    zstd_threads: zstd_threads.unwrap_or(cfg.defaults.zstd_threads),
                    threads: thread_count,
                    exclude: &all_excludes,
                    use_checksum: use_cs,
                    dedup: dedup || cfg.create.dedup,
                },
                &out,
            )?;
        }

        Commands::Prune { dir, keep, max_age } => {
            let keep_n = keep.unwrap_or(cfg.prune.keep_last);
            let age = max_age.unwrap_or(cfg.prune.max_age_days);
            prune::prune(&dir, keep_n, age, &out)?;
        }

        Commands::Merge {
            indexes,
            output,
            split_gb,
            compress,
            zstd_level,
            zstd_threads,
            overwrite,
            sign: sign_flag,
        } => {
            let signer = load_signer(&sign_flag)?;
            let compress_str = compress.as_deref().unwrap_or(&cfg.defaults.compress);
            let algo = CompressionAlgo::parse(compress_str)?;
            let zstd_lvl = zstd_level.unwrap_or(cfg.defaults.zstd_level);
            let split =
                (split_gb.unwrap_or(cfg.defaults.split_gb) * 1024.0 * 1024.0 * 1024.0) as u64;
            merge::merge(
                merge::MergeParams {
                    indexes: &indexes,
                    output_dir: &output,
                    split_bytes: split,
                    algo,
                    zstd_level: zstd_lvl,
                    zstd_threads: zstd_threads.unwrap_or(cfg.defaults.zstd_threads),
                    workers: cfg.defaults.threads,
                    overwrite,
                    sign_key: signer.as_ref(),
                },
                &out,
            )?;
        }

        Commands::Keygen { name } => {
            let (sec, public) = sign::keygen(&name)?;
            println!("{} {}", "Secret key:".cyan().bold(), sec.display());
            println!("{} {}", "Public key:".cyan().bold(), public.display());
            println!();
            println!(
                "  Keep the secret key private. Sign with:   archivum create ... --sign {}",
                sec.display()
            );
            println!(
                "  Require the signature when reading with:  archivum --pubkey {} verify ...",
                public.display()
            );
        }

        Commands::Repair { dir, compression } => {
            utils::print_banner(&out);
            repair::repair(&dir, Some(&compression), &out)?;
        }

        Commands::Completions { shell } => {
            completions::generate_completions(&shell)?;
        }

        Commands::Setup => {
            Config::setup_interactive()?;
        }

        Commands::Config => {
            cfg.print();
            if let Some(p) = config::config_path() {
                if !p.exists() {
                    println!();
                    println!(
                        "  {} Config file does not exist yet. Run {} to create it.",
                        "Note:".yellow(),
                        "archivum setup".cyan()
                    );
                }
            }
        }
    }

    Ok(())
}
