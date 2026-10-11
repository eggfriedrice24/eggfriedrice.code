//! The mirror of a project's refs: at the first snapshot of a git root in a turn, the
//! store fetches `refs/heads/*`, `refs/tags/*`, `refs/stash` and a detached `HEAD` of
//! the project into `refs/efr-mirror/<conversation>/<turn>/`, so the commits that the
//! refs named before the turn stay in the store whatever the turn does to them. The
//! fetch names the git dir that the daemon trusts ([`GitDirs`]), and its
//! `upload-pack` runs hardened ([`ProjectGit::upload_pack`]).
//!
//! No mirror, and the refs of the root are not covered, when: the daemon named no
//! trusted git dir for a root with a `.git`; [`ProjectGit::open`] refuses the dirs;
//! the project is shallow; `Limits::mirror_max_bytes` is 0 or below the size of the
//! project's object dir; the fetch fails; or the fetch adds more than
//! `mirror_max_bytes` to the store, in which case the turn's mirror refs go again.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::Path;

use efr_protocol::{ConversationId, TurnId};

use crate::capture::Limits;
use crate::project_git::{GitDirs, Pins, ProjectGit, ProjectRefs};
use crate::runner::Run;
use crate::snapshots::{Root, Snapshots};
use crate::store::Store;

/// Why a root has no mirror in a turn.
pub(crate) const MIRROR_UNTRUSTED: &str = "the git dir is not trusted";
pub(crate) const MIRROR_SHALLOW: &str = "shallow";
pub(crate) const MIRROR_OFF: &str = "snapshot.mirror_max_mib is 0";
pub(crate) const MIRROR_TOO_LARGE: &str = "over snapshot.mirror_max_mib";
pub(crate) const MIRROR_FETCH_FAILED: &str = "the fetch failed";
pub(crate) const MIRROR_OVER_BUDGET: &str = "mirror over budget";

/// What the first snapshot of a git root in a turn took of the project's refs.
#[derive(Debug, Clone, Default)]
pub(crate) struct Mirror {
    /// The git dirs and their pins, when efr could open them.
    pub(crate) git: Option<(GitDirs, Pins)>,
    /// The refs at the snapshot, and `HEAD`.
    pub(crate) refs: ProjectRefs,
    /// `None` when the store holds the mirror, else why not.
    pub(crate) missing: Option<&'static str>,
}

/// `refs/efr-mirror/<conversation>/<turn>/`.
pub(crate) fn mirror_prefix(conversation: ConversationId, turn: TurnId) -> String {
    format!("refs/efr-mirror/{conversation}/{turn}/")
}

impl Snapshots {
    /// Takes the mirror of `root` for the turn into `store`. `None` for a root that is
    /// no git work tree. The caller holds the store's gate.
    pub(crate) async fn take_mirror(
        &self,
        conversation: ConversationId,
        turn: TurnId,
        root: &Root,
        store: &Store,
        limits: &Limits,
    ) -> Option<Mirror> {
        let Some(dirs) = root.git() else {
            let dot_git = root.path().join(".git");
            let looked = self
                .runner()
                .blocking(root.path(), move || fs::symlink_metadata(dot_git).is_ok())
                .await;
            return looked
                .unwrap_or(false)
                .then(|| Mirror { missing: Some(MIRROR_UNTRUSTED), ..Mirror::default() });
        };
        let opened = {
            let runner = self.runner().clone();
            let dirs = dirs.clone();
            self.runner()
                .blocking(root.path(), move || ProjectGit::open(&runner, &dirs, None))
                .await
        };
        let project = match opened {
            Ok(Ok(project)) => project,
            Ok(Err(refusal)) => {
                return Some(Mirror { missing: Some(refusal.reason()), ..Mirror::default() });
            }
            Err(error) => {
                tracing::warn!(error = %efr_stdx::with_causes(&error), root = %root.path().display(), "the git dir of a project could not be read");
                return Some(Mirror { missing: Some(MIRROR_FETCH_FAILED), ..Mirror::default() });
            }
        };
        let mut mirror = Mirror { git: Some((dirs.clone(), project.pins())), ..Mirror::default() };
        mirror.refs = match project.refs().await {
            Ok(refs) => refs,
            Err(error) => {
                tracing::warn!(error = %efr_stdx::with_causes(&error), root = %root.path().display(), "the refs of a project could not be read");
                mirror.missing = Some(MIRROR_FETCH_FAILED);
                return Some(mirror);
            }
        };
        mirror.missing =
            self.fetch(conversation, turn, &project, store, limits, &mut mirror.refs).await;
        Some(mirror)
    }

    /// Fetches the refs of `refs` into the turn's mirror; on success `refs` holds the
    /// values that the mirror got. Returns why there is no mirror, or `None`.
    async fn fetch(
        &self,
        conversation: ConversationId,
        turn: TurnId,
        project: &ProjectGit,
        store: &Store,
        limits: &Limits,
        refs: &mut ProjectRefs,
    ) -> Option<&'static str> {
        if limits.mirror_max_bytes == 0 {
            return Some(MIRROR_OFF);
        }
        let probed = {
            let project = project.clone();
            let most = limits.mirror_max_bytes;
            self.runner()
                .blocking(&project.dirs().common_dir.clone(), move || {
                    (project.shallow(), project.objects_bytes(most))
                })
                .await
        };
        let (shallow, bytes) = match probed {
            Ok(probed) => probed,
            Err(_) => return Some(MIRROR_FETCH_FAILED),
        };
        if shallow {
            return Some(MIRROR_SHALLOW);
        }
        if bytes > limits.mirror_max_bytes {
            return Some(MIRROR_TOO_LARGE);
        }
        let prefix = mirror_prefix(conversation, turn);
        let mut specs: Vec<String> = refs
            .refs
            .keys()
            .map(|name| format!("+{name}:{prefix}{}", name.trim_start_matches("refs/")))
            .collect();
        if refs.head_commit.is_some() {
            specs.push(format!("+HEAD:{prefix}HEAD"));
        }
        if specs.is_empty() {
            return None;
        }
        let packs_before = pack_bytes(store);
        let mut args: Vec<OsString> = [
            "-c",
            "fetch.unpackLimit=1",
            "-c",
            "fetch.writeCommitGraph=false",
            "fetch",
            "--quiet",
            "--no-tags",
            "--no-write-fetch-head",
            "--no-recurse-submodules",
            "--no-auto-gc",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        let mut upload = OsString::from("--upload-pack=");
        upload.push(ProjectGit::upload_pack());
        args.push(upload);
        args.push(project.dirs().git_dir.as_os_str().to_owned());
        args.extend(specs.into_iter().map(OsString::from));
        let fetched = self.runner().run(store, &args, Run { stderr: true, ..Run::default() }).await;
        match fetched {
            Ok(output) if output.success => {}
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let error =
                    stderr.lines().find(|line| line.starts_with("fatal:")).unwrap_or_default();
                tracing::warn!(root = %store.root().display(), error, "the mirror of a project's refs failed");
                return Some(MIRROR_FETCH_FAILED);
            }
            Err(error) => {
                tracing::warn!(error = %efr_stdx::with_causes(&error), root = %store.root().display(), "the mirror of a project's refs failed");
                return Some(MIRROR_FETCH_FAILED);
            }
        }
        let added = pack_bytes(store).saturating_sub(packs_before);
        let mirrored = self.mirror_refs(store, &prefix).await;
        if added > limits.mirror_max_bytes {
            self.drop_mirror(store, &mirrored).await;
            return Some(MIRROR_OVER_BUDGET);
        }
        // NOTE: a ref can move between the listing and the fetch; the record holds
        // what the store has.
        let mut values = BTreeMap::new();
        for (name, id) in &mirrored {
            let Some(rest) = name.strip_prefix(&prefix) else { continue };
            if rest == "HEAD" {
                refs.head_commit = Some(id.clone());
            } else {
                values.insert(format!("refs/{rest}"), id.clone());
            }
        }
        if values.len() != refs.refs.len() {
            tracing::warn!(root = %store.root().display(), "the mirror of a project's refs is not complete");
            self.drop_mirror(store, &mirrored).await;
            return Some(MIRROR_FETCH_FAILED);
        }
        refs.refs = values;
        None
    }

    /// The refs below `prefix` in `store`: name to object id.
    pub(crate) async fn mirror_refs(&self, store: &Store, prefix: &str) -> Vec<(String, String)> {
        let out = self
            .runner()
            .checked(
                store,
                "for-each-ref",
                &["for-each-ref", "--format=%(objectname) %(refname)", prefix],
                Run::default(),
            )
            .await
            .unwrap_or_default();
        String::from_utf8_lossy(&out)
            .lines()
            .filter_map(|line| line.split_once(' '))
            .map(|(id, name)| (name.to_owned(), id.to_owned()))
            .collect()
    }

    async fn drop_mirror(&self, store: &Store, refs: &[(String, String)]) {
        let commands: String = refs.iter().map(|(name, _)| format!("delete {name}\n")).collect();
        let deleted = self
            .runner()
            .checked(
                store,
                "update-ref",
                &["update-ref", "--stdin"],
                Run { stdin: Some(commands.into_bytes()), ..Run::default() },
            )
            .await;
        if let Err(error) = deleted {
            tracing::warn!(error = %efr_stdx::with_causes(&error), root = %store.root().display(), "the mirror refs of a turn could not be deleted");
        }
    }
}

/// The bytes of the pack files of `store`.
fn pack_bytes(store: &Store) -> u64 {
    let dir = store.git_dir().join("objects/pack");
    let Ok(entries) = fs::read_dir(dir) else { return 0 };
    entries
        .filter_map(Result::ok)
        .filter(|entry| Path::new(&entry.file_name()).extension().is_some_and(|ext| ext == "pack"))
        .filter_map(|entry| entry.metadata().ok())
        .map(|metadata| metadata.len())
        .sum()
}
