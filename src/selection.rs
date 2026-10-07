//! Pure launch resolution and menu transitions.
use super::Entry;
use std::ffi::OsStr;
use std::path::Path;

/// Resolve exact identities first, then unambiguous legacy short names.
pub(super) fn find_entry(entries: &[Entry], want: &OsStr) -> Result<Option<usize>, String> {
    let path = Path::new(want);
    if let Some(i) = entries.iter().position(|e| e.id == path) {
        return Ok(Some(i));
    }
    // A path that no longer exists must not fall back to a different project.
    if path.components().count() != 1 {
        return Ok(None);
    }
    let exact = entries.iter().any(|e| OsStr::new(&e.name) == want);
    let short = if exact {
        want
    } else {
        path.file_stem().unwrap_or(want)
    };
    let mut matches = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| OsStr::new(&e.name) == short);
    let Some((index, _)) = matches.next() else {
        return Ok(None);
    };
    if matches.next().is_some() {
        let candidates = entries
            .iter()
            .filter(|e| OsStr::new(&e.name) == short)
            .map(|e| e.id.display().to_string())
            .collect::<Vec<_>>()
            .join("\n  ");
        return Err(format!(
            "Multiple projects share the name {}. Specify a relative path:\n  {candidates}",
            want.to_string_lossy(),
        ));
    }
    Ok(Some(index))
}

pub(super) fn default_index(entries: &[Entry], default: &str) -> Result<usize, String> {
    if entries.is_empty() {
        return Err("No launchable projects.".into());
    }
    Ok(find_entry(entries, OsStr::new(default))?.unwrap_or(0))
}

#[derive(Debug, PartialEq)]
pub(super) enum Action {
    Launch(usize),
    Create,
    Delete,
}

pub(super) fn action(
    entries: &[Entry],
    default: usize,
    input: Option<&str>,
) -> Result<Action, String> {
    if entries.is_empty() {
        return Err("No launchable projects.".into());
    }
    let Some(input) = input.filter(|s| !s.is_empty()) else {
        return entries
            .get(default)
            .map(|_| Action::Launch(default))
            .ok_or_else(|| "No default project.".into());
    };
    match input {
        "n" | "N" => Ok(Action::Create),
        "d" | "D" => Ok(Action::Delete),
        value => {
            if let Ok(n) = value.parse::<usize>()
                && let Some(i) = n.checked_sub(1).filter(|&i| i < entries.len())
            {
                return Ok(Action::Launch(i));
            }
            find_entry(entries, OsStr::new(value))?
                .map(Action::Launch)
                .ok_or_else(|| format!("No such launch: {value}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries() -> Vec<Entry> {
        [
            "projects/robot_v1.1/main.yaml",
            "experiments/probe/projects/robot_v1.1/main.yaml",
        ]
        .into_iter()
        .map(|id| Entry {
            id: id.into(),
            name: "robot_v1.1".into(),
            path: Path::new("bundle").join(id),
        })
        .collect()
    }

    #[test]
    fn duplicate_names_require_identity_for_arguments_and_defaults() {
        let es = entries();
        for short in ["robot_v1.1", "robot_v1.1.toml"] {
            assert!(find_entry(&es, OsStr::new(short)).is_err());
            assert!(default_index(&es, short).is_err());
            let mut args = vec![short.into()];
            assert!(super::super::take_named(&es, &mut args).is_err());
            assert_eq!(args.len(), 1);
        }
        let id = es[1].id.to_str().unwrap();
        assert_eq!(default_index(&es, id).unwrap(), 1);
        assert_eq!(action(&es, 0, Some(id)).unwrap(), Action::Launch(1));
        let mut args = vec![id.into(), "--log-level".into()];
        assert_eq!(
            super::super::take_named(&es, &mut args).unwrap(),
            Some(es[1].path.clone())
        );
        assert_eq!(args, vec![std::ffi::OsString::from("--log-level")]);
        assert_eq!(
            find_entry(&es, OsStr::new("missing/robot_v1.1.toml")).unwrap(),
            None
        );
        assert_eq!(
            find_entry(&es[..1], OsStr::new("robot_v1.1")).unwrap(),
            Some(0)
        );
    }

    #[test]
    fn menu_handles_removal_of_default_and_last_entry() {
        let mut es = entries();
        let default = es[1].id.to_str().unwrap().to_owned();
        assert_eq!(action(&es, 1, None).unwrap(), Action::Launch(1));
        assert_eq!(action(&es, 1, Some("1")).unwrap(), Action::Launch(0));
        assert_eq!(action(&es, 1, Some("d")).unwrap(), Action::Delete);
        assert_eq!(action(&es, 1, Some("n")).unwrap(), Action::Create);
        assert!(action(&es, 1, Some("0")).is_err());
        let _ = es.pop();
        let index = default_index(&es, &default).unwrap();
        assert_eq!(index, 0);
        assert_eq!(action(&es, index, Some("")).unwrap(), Action::Launch(0));
        let _ = es.pop();
        assert!(default_index(&es, &default).is_err());
        assert!(action(&es, 0, None).is_err());
    }
}
