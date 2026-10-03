//! Double-click entry point for a bundle (a single exe, renamed to `<name>` when distributed).
//!
//! The real work of a bundle is `reiny run <launch config> --bin-dir bin`, but installer
//! shortcuts and Velopack can only point at **one argument-free exe**. Velopack also
//! requires its hook to run at the **very start** of `main`, and the only other `main` in
//! the bundle belongs to reiny (a crate we do not own). Those two constraints are the whole
//! reason this thin shim exists: it is the single entry for install/update and launch selection.
//!
//! A bundle has the same shape as the repository, so everything launchable is every
//! `projects/<name>/launch.yaml` (a **project**) beside the executable. Projects can be
//! duplicated and deleted from this screen. A project is self-contained in one directory and
//! every relative path inside it is based on that directory, so duplication is a pure
//! recursive copy that never rewrites a path.
//!
//! Which launch runs is chosen by an argument (`<name> my_test`) or, without one, by number.
//! The default is `default.launch`, which identifies a bundle-relative config path (or a
//! unique legacy short name). With no default, the first entry is selected. This shim passes
//! `--bin-dir bin` explicitly; reiny's renamed-executable entry does not inspect these arguments.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod selection;
use selection::find_entry;

#[cfg(test)]
use std::ffi::OsStr;
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

/// Environment variable the dynamic loader searches for libraries. The bundle's `bin/` is
/// prepended to it.
///
/// Windows looks in the loaded exe's directory first, so DLLs in `bin/` already resolve, but
/// ELF and Mach-O loaders do not, so the path is supplied here.
const LIB_PATH_VAR: &str = if cfg!(windows) {
    "PATH"
} else if cfg!(target_os = "macos") {
    "DYLD_LIBRARY_PATH"
} else {
    "LD_LIBRARY_PATH"
};

/// Separator for `LIB_PATH_VAR`.
const LIB_PATH_SEP: &str = if cfg!(windows) { ";" } else { ":" };

/// File (one line) holding the auto-update feed URL. Without it, no update check is made.
const UPDATE_URL_FILE: &str = "update.url";

/// Bundle-relative launch path selected by Enter; unique legacy names are also accepted.
///
/// Kept separate from the bundle name (= executable name): a bundle holds every launch
/// config, so its name is robot-independent and cannot be used to look up a default.
const DEFAULT_LAUNCH_FILE: &str = "default.launch";

/// Directory holding the user's projects (directly under the bundle / repository root).
const PROJECTS_DIR: &str = "projects";

/// Launch config inside a project directory. The **fixed name** is the point: the directory
/// carries the name, and since the contents do not depend on the file name, a directory
/// duplicates without renaming or rewriting.
const PROJECT_LAUNCH: &str = "launch.yaml";

fn main() -> ExitCode {
    // Velopack may exit or restart this process for install/update/uninstall hooks, so it
    // must run first. It does nothing for a plain (not installed) bundle.
    velopack::VelopackApp::build().run();

    match launch() {
        Ok(code) => code,
        Err(msg) => {
            eprintln!("{msg}");
            ExitCode::FAILURE
        }
    }
}

/// Pick a launch config, start it with the adjacent reiny (or the system one), and return
/// its exit code.
fn launch() -> Result<ExitCode, String> {
    let exe = std::env::current_exe()
        .map_err(|e| format!("Cannot determine the path of this executable: {e}"))?;
    let dir = exe
        .parent()
        .ok_or_else(|| format!("The executable has no parent directory: {}", exe.display()))?;
    // Update before launching. Applying an update restarts this process, so it never returns.
    update(dir);

    let default = std::fs::read_to_string(dir.join(DEFAULT_LAUNCH_FILE))
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let mut args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let config = pick_config(dir, &default, &mut args)?;
    let bindir = dir.join("bin");
    let reiny = reiny_path(dir);

    println!("==> Launching {}", config.display());
    let mut cmd = Command::new(&reiny);
    let _ = cmd
        .arg("run")
        .arg(&config)
        .arg("--bin-dir")
        .arg(&bindir)
        .args(&args)
        .env(
            LIB_PATH_VAR,
            prepend(&bindir, std::env::var_os(LIB_PATH_VAR)),
        );

    let status = cmd
        .status()
        .map_err(|e| format!("Cannot start reiny: {} ({e})", reiny.display()))?;
    let code = status.code().unwrap_or(1);
    Ok(ExitCode::from(u8::try_from(code).unwrap_or(1)))
}

// ---- Launchable entries -----------------------------------------------------

/// One launchable entry (`projects/<name>/launch.yaml`).
struct Entry {
    /// Bundle-relative identity, distinct from the display name.
    id: PathBuf,
    /// Display name and legacy short argument, accepted only when unambiguous.
    name: String,
    /// The launch config actually passed to `reiny run`.
    path: PathBuf,
}

/// Direct children of `dir`, sorted by name. Empty if unreadable.
fn sorted_entries(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    out.sort();
    out
}

/// Launchable entries (`projects/*/launch.yaml`), sorted by name.
fn list_entries(root: &Path) -> Vec<Entry> {
    let mut dirs = sorted_entries(&root.join(PROJECTS_DIR));
    // Only explicitly bundled experiments are present here; normal bundles contain projects only.
    for experiment in sorted_entries(&root.join("experiments")) {
        dirs.extend(sorted_entries(&experiment.join(PROJECTS_DIR)));
    }
    let mut entries: Vec<_> = dirs
        .into_iter()
        .filter(|p| p.is_dir())
        .map(|p| Entry {
            id: p
                .join(PROJECT_LAUNCH)
                .strip_prefix(root)
                .unwrap_or(&p)
                .to_path_buf(),
            name: p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            path: p.join(PROJECT_LAUNCH),
        })
        .filter(|e| e.path.is_file())
        .collect();
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    entries
}

/// If the first argument names a launch, **consume** it as the selection and return it.
///
/// If it is not a name, nothing is taken: mistaking a flag such as `--log-level debug` for a
/// launch name would silently drop it instead of passing it on to reiny.
fn take_named(entries: &[Entry], args: &mut Vec<OsString>) -> Result<Option<PathBuf>, String> {
    let Some(first) = args.first() else {
        return Ok(None);
    };
    let Some(i) = find_entry(entries, first)? else {
        return Ok(None);
    };
    let _ = args.remove(0);
    Ok(Some(entries[i].path.clone()))
}

// ---- Selection menu ---------------------------------------------------------

/// Decide which launch config to run. A name given as an argument is consumed.
///
/// A single entry is used silently. With several, the user picks by number; empty input
/// selects the default (the launch `default` points at, else the first). End of input (EOF)
/// also proceeds with the default, so shortcut or CI launches do not stall. `n` / `d`
/// duplicate / delete a project; both rebuild the list and return to the menu.
fn pick_config(root: &Path, default: &str, args: &mut Vec<OsString>) -> Result<PathBuf, String> {
    let entries = list_entries(root);
    if entries.is_empty() {
        return Err(format!(
            "No launch config found: {}\n\
             (this executable selects what to launch from the adjacent {PROJECTS_DIR}/*/{PROJECT_LAUNCH})",
            root.display()
        ));
    }
    if let Some(named) = take_named(&entries, args)? {
        return Ok(named);
    }
    if entries.len() == 1 {
        return Ok(entries[0].path.clone());
    }

    let mut entries = entries;
    loop {
        let default_idx = selection::default_index(&entries, default)?;
        println!("Choose a launch to start:");
        for (i, e) in entries.iter().enumerate() {
            let mark = if i == default_idx { "  (default)" } else { "" };
            println!("  {:>2}) {}  ({}){mark}", i + 1, e.name, e.id.display());
        }
        println!("   n) Create a project (duplicate)   d) Delete a project");

        let line = read_line(&format!("Number [{}]: ", default_idx + 1));
        match selection::action(&entries, default_idx, line.as_deref()) {
            Ok(selection::Action::Launch(i)) => return Ok(entries[i].path.clone()),
            Ok(selection::Action::Create) => {
                if let Err(e) = new_project(root, &entries) {
                    eprintln!("{e}");
                }
                entries = list_entries(root);
            }
            Ok(selection::Action::Delete) => {
                if let Err(e) = delete_project(&entries) {
                    eprintln!("{e}");
                }
                entries = list_entries(root);
            }
            Err(e) => eprintln!("{e}"),
        }
    }
}

/// Let the user pick one by number. Empty input, EOF, or out of range yields `None` (cancel).
fn choose<'a>(title: &str, items: &[&'a Entry]) -> Option<&'a Entry> {
    if items.is_empty() {
        println!("Nothing to choose from.");
        return None;
    }
    println!("{title}");
    for (i, e) in items.iter().enumerate() {
        println!("  {:>2}) {}  ({})", i + 1, e.name, e.id.display());
    }
    let n = read_line("Number: ")?.parse::<usize>().ok()?;
    items.get(n.checked_sub(1)?).copied()
}

// ---- Project creation / deletion --------------------------------------------

/// Whether `s` is usable as a project name (= safe as a single path component).
///
/// Rejecting separators and `..` keeps every project inside `projects/`.
fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('.')
        && s.chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// Recursively copy a directory (project duplication).
fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let (from, to) = (entry.path(), dst.join(entry.file_name()));
        if entry.file_type()?.is_dir() {
            copy_dir_all(&from, &to)?;
        } else {
            let _ = std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// Create `projects/<name>/` from `src` and return that directory.
///
/// The directory is duplicated as is: relative paths inside it are based only on its own
/// directory, so not a single character is rewritten.
fn create_project(root: &Path, src: &Entry, name: &str) -> Result<PathBuf, String> {
    if !valid_name(name) {
        return Err(format!(
            "Invalid project name: {name}\n(only alphanumerics and - _ . are allowed; no leading .)"
        ));
    }
    // Preserve directory depth so config-relative references survive copying an experiment.
    let parent = src
        .path
        .parent()
        .and_then(Path::parent)
        .filter(|p| p.starts_with(root))
        .ok_or_else(|| "The source is not a project inside the bundle".to_string())?;
    let dir = parent.join(name);
    if dir.exists() {
        return Err(format!("Already exists: {}", dir.display()));
    }
    let from = src
        .path
        .parent()
        .ok_or_else(|| format!("The source has no directory: {}", src.path.display()))?;

    copy_dir_all(from, &dir).map_err(|e| format!("Cannot duplicate: {e}"))?;
    Ok(dir)
}

/// Pick a source and create `projects/<name>/` from it.
fn new_project(root: &Path, entries: &[Entry]) -> Result<(), String> {
    let all: Vec<&Entry> = entries.iter().collect();
    let Some(src) = choose("Choose a project to duplicate:", &all) else {
        return Ok(());
    };
    let Some(name) = read_line("New project name: ") else {
        return Ok(());
    };
    let dir = create_project(root, src, &name)?;
    println!("==> Created {}", dir.display());
    Ok(())
}

/// Pick a project and delete its whole directory (with confirmation).
fn delete_project(entries: &[Entry]) -> Result<(), String> {
    let all: Vec<&Entry> = entries.iter().collect();
    let Some(target) = choose("Choose a project to delete:", &all) else {
        return Ok(());
    };
    let dir = target
        .path
        .parent()
        .ok_or_else(|| format!("No directory: {}", target.path.display()))?;
    if !confirm(&format!("Delete {}. Are you sure?", dir.display())) {
        return Ok(());
    }
    std::fs::remove_dir_all(dir).map_err(|e| format!("Cannot delete: {e}"))?;
    println!("==> Deleted {}", dir.display());
    Ok(())
}

// ---- Input ------------------------------------------------------------------

/// Print a prompt and read one line. Empty input and EOF are both `None` ("default" / "cancel").
fn read_line(prompt: &str) -> Option<String> {
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).unwrap_or(0) == 0 {
        return None;
    }
    let s = line.trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

/// A `[y/N]` confirmation. EOF (non-interactive) means "no", so unattended launches never
/// delete or update on their own.
fn confirm(prompt: &str) -> bool {
    let Some(ans) = read_line(&format!("{prompt} [y/N]: ")) else {
        return false;
    };
    matches!(ans.to_ascii_lowercase().as_str(), "y" | "yes")
}

// ---- Auto-update ------------------------------------------------------------

/// If `update.url` exists, check the feed and, when an update is available, ask for consent
/// and apply it.
///
/// Applying restarts the process, so this does not return in that case. No network, not
/// installed (portable layout), and similar conditions are all treated as "do nothing":
/// failing to launch because an update failed would defeat the purpose, so this function
/// never returns an error.
fn update(dir: &Path) {
    let Ok(url) = std::fs::read_to_string(dir.join(UPDATE_URL_FILE)) else {
        return;
    };
    let url = url.trim();
    if url.is_empty() {
        return;
    }
    let source = velopack::sources::AutoSource::new(url);
    let Ok(um) = velopack::UpdateManager::new(source, None, None) else {
        return; // Not installed (= not eligible for updates).
    };
    let Ok(velopack::UpdateCheck::UpdateAvailable(info)) = um.check_for_updates() else {
        return;
    };
    let ver = &info.TargetFullRelease.Version;
    println!(
        "A new version {ver} is available (current {}).",
        um.get_current_version_as_string()
    );
    if !confirm("Update now?") {
        return;
    }
    println!("Downloading...");
    if let Err(e) = um.download_updates(&info, None) {
        eprintln!("Cannot download the update: {e}");
        return;
    }
    // On success this process exits and restarts as the new version.
    if let Err(e) = um.apply_updates_and_restart(&info.TargetFullRelease) {
        eprintln!("Cannot apply the update: {e}");
    }
}

// ---- Paths ------------------------------------------------------------------

/// The bundled reiny; falls back to the one on PATH if missing (a safety net for a bundle
/// that forgot to include it).
fn reiny_path(dir: &Path) -> PathBuf {
    let bundled = dir.join(format!("reiny{}", std::env::consts::EXE_SUFFIX));
    if bundled.is_file() {
        bundled
    } else {
        PathBuf::from("reiny")
    }
}

/// Build a search path with `head` in front of the existing value.
fn prepend(head: &Path, existing: Option<OsString>) -> OsString {
    let mut out = head.as_os_str().to_os_string();
    if let Some(old) = existing
        && !old.is_empty()
    {
        out.push(LIB_PATH_SEP);
        out.push(old);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn experimental_projects_keep_their_depth_when_listed_and_copied() {
        let root = bundle("experiment", &["daily"]);
        let parent = root.join("experiments/2026-09-11_probe/projects");
        let original = parent.join("robot_v1.1_probe");
        std::fs::create_dir_all(&original).unwrap();
        std::fs::write(original.join(PROJECT_LAUNCH), "launch:\n").unwrap();
        let entries = list_entries(&root);
        let i = find_entry(&entries, OsStr::new("robot_v1.1_probe"))
            .unwrap()
            .unwrap();
        assert_eq!(entries[i].path, original.join(PROJECT_LAUNCH));
        assert_eq!(
            entries[i].id,
            Path::new("experiments/2026-09-11_probe/projects/robot_v1.1_probe/launch.yaml")
        );
        let copied = create_project(&root, &entries[i], "robot_v1.1_copy").unwrap();
        assert_eq!(copied, parent.join("robot_v1.1_copy"));
        assert_eq!(
            std::fs::read(copied.join(PROJECT_LAUNCH)).unwrap(),
            b"launch:\n"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn same_named_projects_are_selected_by_bundle_relative_path() {
        let root = bundle("duplicate-names", &["robot_v1.1"]);
        let id = "experiments/probe/projects/robot_v1.1/launch.yaml";
        let path = root.join(id);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "launch:\n").unwrap();
        let mut args = vec![id.into()];
        assert_eq!(pick_config(&root, "robot_v1.1", &mut args).unwrap(), path);
        assert!(args.is_empty());
        assert!(pick_config(&root, "robot_v1.1", &mut args).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_bundle_bin_dir_goes_in_front_of_an_existing_search_path() {
        let head = Path::new("/bundle/bin");
        assert_eq!(prepend(head, None), OsString::from("/bundle/bin"));
        assert_eq!(
            prepend(head, Some(OsString::from(""))),
            OsString::from("/bundle/bin")
        );
        assert_eq!(
            prepend(head, Some(OsString::from("/usr/lib"))),
            OsString::from(format!("/bundle/bin{LIB_PATH_SEP}/usr/lib")),
        );
    }

    /// Create a bundle-like directory for tests (projects under `projects/`).
    fn bundle(tag: &str, projects: &[&str]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rancher-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for p in projects {
            let pd = dir.join(PROJECTS_DIR).join(p);
            std::fs::create_dir_all(&pd).unwrap();
            std::fs::write(
                pd.join(PROJECT_LAUNCH),
                "launch:\n  sutera-launch: sutera.toml\n",
            )
            .unwrap();
            std::fs::write(pd.join("sutera.toml"), "[controller]\nprogram = 'probe'\n").unwrap();
        }
        dir
    }

    fn entries(names: &[&str]) -> Vec<Entry> {
        names
            .iter()
            .map(|n| Entry {
                id: PathBuf::from(PROJECTS_DIR).join(n).join(PROJECT_LAUNCH),
                name: (*n).to_string(),
                path: PathBuf::from(PROJECTS_DIR).join(n).join(PROJECT_LAUNCH),
            })
            .collect()
    }

    /// A name argument resolves with or without an extension (`<name> sim` and `<name> sim.toml`).
    #[test]
    fn a_config_is_found_by_stem_or_file_name() {
        let es = entries(&["a_sim", "b_real"]);
        assert_eq!(find_entry(&es, OsStr::new("b_real")).unwrap(), Some(1));
        assert_eq!(find_entry(&es, OsStr::new("b_real.toml")).unwrap(), Some(1));
        assert_eq!(find_entry(&es, OsStr::new("nope")).unwrap(), None);
    }

    /// A leading launch name is consumed as the selection and the rest goes to reiny. If it
    /// is not a name, nothing is consumed (`--log-level debug` is not mistaken for a launch).
    #[test]
    fn a_leading_launch_name_is_consumed_but_other_flags_are_not() {
        let es = entries(&["a_sim", "b_real"]);

        let mut args = vec![OsString::from("b_real"), OsString::from("--log-level")];
        assert_eq!(take_named(&es, &mut args).unwrap().unwrap(), es[1].path);
        assert_eq!(args, vec![OsString::from("--log-level")]);

        let mut args = vec![OsString::from("--log-level"), OsString::from("debug")];
        assert!(take_named(&es, &mut args).unwrap().is_none());
        assert_eq!(args.len(), 2, "flags must not be consumed");

        let mut args: Vec<OsString> = Vec::new();
        assert!(take_named(&es, &mut args).unwrap().is_none());
    }

    /// A single-config bundle launches directly without a menu (never falls into interaction).
    #[test]
    fn a_single_launch_config_is_used_without_asking() {
        let dir = bundle("pick", &["only_sim"]);

        let mut args = vec![OsString::from("--log-level")];
        let picked = pick_config(&dir, "whatever", &mut args).unwrap();
        assert_eq!(
            picked,
            dir.join(PROJECTS_DIR).join("only_sim").join(PROJECT_LAUNCH)
        );
        assert_eq!(args.len(), 1);

        // A bundle with no config at all fails and says why.
        std::fs::remove_dir_all(dir.join(PROJECTS_DIR)).unwrap();
        assert!(pick_config(&dir, "whatever", &mut args).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Projects are listed by directory name in order and can be looked up by name. A
    /// directory without launch.yaml does not appear.
    #[test]
    fn projects_are_listed_by_directory_name() {
        let dir = bundle("list", &["b_sim", "a_sim"]);
        std::fs::create_dir_all(dir.join(PROJECTS_DIR).join("shared")).unwrap();
        let es = list_entries(&dir);
        let names: Vec<&str> = es.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["a_sim", "b_sim"]);
        assert_eq!(
            es[1].path,
            dir.join(PROJECTS_DIR).join("b_sim").join(PROJECT_LAUNCH)
        );

        // A project name given as an argument also resolves.
        let mut args = vec![OsString::from("b_sim")];
        assert_eq!(take_named(&es, &mut args).unwrap().unwrap(), es[1].path);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Duplicating a project copies the directory verbatim; no character changes.
    #[test]
    fn a_project_is_duplicated_verbatim() {
        let dir = bundle("dup", &["src_proj"]);
        let es = list_entries(&dir);
        let made = create_project(&dir, &es[0], "copy_proj").unwrap();

        for f in [PROJECT_LAUNCH, "sutera.toml"] {
            assert_eq!(
                std::fs::read_to_string(made.join(f)).unwrap(),
                std::fs::read_to_string(es[0].path.parent().unwrap().join(f)).unwrap(),
                "{f} was rewritten"
            );
        }
        // The same name cannot be created twice. Separators and .. are not allowed in names.
        assert!(create_project(&dir, &es[0], "copy_proj").is_err());
        assert!(create_project(&dir, &es[0], "../evil").is_err());
        assert!(create_project(&dir, &es[0], "a/b").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
