use std::path::Path;

use anyhow::{anyhow, Context};

use radicle::cob::patch::{MergeTarget, PatchId, PatchMut, Patches};
use radicle::git;
use radicle::git::raw::Oid;
use radicle::prelude::*;
use radicle::storage::git::Repository;

use crate::terminal as term;
use crate::terminal::args::Error;

use super::common;
use super::{Options, Update};

const PATCH_MSG: &str = r#"
<!--
Please enter a patch message for your changes. An empty
message aborts the patch proposal.

The first line is the patch title. The patch description
follows, and must be separated with a blank line, just
like a commit message. Markdown is supported in the title
and description.
-->
"#;

const REVISION_MSG: &str = r#"
<!--
Please enter a comment message for your patch update. Leaving this
blank is also okay.
-->
"#;

/// Run patch creation.
pub fn run(
    storage: &Repository,
    profile: &Profile,
    workdir: &git::raw::Repository,
    options: Options,
) -> anyhow::Result<()> {
    let project = storage.project_of(&profile.public_key).context(format!(
        "couldn't load project {} from local state",
        storage.id
    ))?;

    term::headline(&format!(
        "🌱 Creating patch for {}",
        term::format::highlight(&project.name)
    ));

    let signer = term::signer(profile)?;
    let mut patches = Patches::open(profile.public_key, storage)?;

    // `HEAD`; This is what we are proposing as a patch.
    let head = workdir.head()?;
    let head_oid = head.target().ok_or(anyhow!("invalid HEAD ref; aborting"))?;
    let head_commit = workdir.find_commit(head_oid)?;
    let head_branch = head
        .shorthand()
        .ok_or(anyhow!("cannot create patch from detached head; aborting"))?;

    // Make sure the `HEAD` commit can be found in the monorepo. Otherwise there
    // is no way for anyone to merge this patch.
    let mut spinner = term::spinner(format!(
        "Looking for HEAD ({}) in storage...",
        term::format::secondary(term::format::oid(head_oid))
    ));
    if storage.commit(head_oid.into()).is_err() {
        if !options.push {
            spinner.failed();
            term::blank();

            return Err(Error::WithHint {
                err: anyhow!("Current branch head was not found in storage"),
                hint: "hint: run `git push rad` and try again",
            }
            .into());
        }
        spinner.message("Pushing HEAD to storage...");

        let output = git::run::<_, _, &str, &str>(Path::new("."), ["push", "rad"], [])?;
        if options.verbose {
            spinner.finish();
            term::blob(output);
        }
    }
    spinner.finish();

    // Determine the merge target for this patch. This can ben any tracked remote's "default"
    // branch, as well as your own (eg. `rad/master`).
    let mut spinner = term::spinner("Analyzing remotes...");
    let targets =
        common::find_merge_targets(&head_oid, project.default_branch.as_refstr(), storage)?;

    // eg. `refs/namespaces/<peer>/refs/heads/master`
    let (target_peer, target_oid) = match targets.not_merged.as_slice() {
        [] => {
            spinner.message("All tracked peers are up to date.");
            return Ok(());
        }
        [target] => target,
        _ => {
            // TODO: Let user select which branch to use as a target.
            todo!();
        }
    };
    // TODO: Tell user how many peers don't have this change.
    spinner.finish();

    // TODO: Handle case where `rad/master` isn't up to date with the target.
    // In that case we should warn the user that their master branch is not up
    // to date, and error out, unless the user specifies manually the merge
    // base.

    // The merge base is basically the commit at which the histories diverge.
    let base_oid = workdir.merge_base((*target_oid).into(), head_oid)?;
    let commits = common::patch_commits(workdir, &base_oid, &head_oid)?;

    let patch = match &options.update {
        Update::No => None,
        Update::Any => {
            let mut spinner = term::spinner("Finding patches to update...");
            let mut result = common::find_unmerged_with_base(
                head_oid,
                **target_oid,
                base_oid,
                &patches,
                workdir,
            )?;

            if let Some((id, patch, clock)) = result.pop() {
                if result.is_empty() {
                    spinner.message(format!(
                        "Found existing patch {} {}",
                        term::format::tertiary(term::format::cob(&id)),
                        term::format::italic(patch.title())
                    ));
                    spinner.finish();
                    term::blank();

                    Some((id, PatchMut::new(id, patch, clock, &mut patches)))
                } else {
                    spinner.failed();
                    term::blank();
                    anyhow::bail!("More than one patch available to update, please specify an id with `rad patch --update <id>`");
                }
            } else {
                spinner.failed();
                term::blank();
                anyhow::bail!("No patches found that share a base, please create a new patch or specify the patch id manually");
            }
        }
        Update::Patch(id) => {
            if let Ok(patch) = patches.get_mut(id) {
                Some((*id, patch))
            } else {
                anyhow::bail!("Patch `{}` not found", id);
            }
        }
    };

    if let Some((id, patch)) = patch {
        if term::confirm("Update?") {
            term::blank();

            return update(patch, id, &base_oid, &head_oid, workdir, options, &signer);
        } else {
            anyhow::bail!("Patch update aborted by user");
        }
    }

    // TODO: List matching working copy refs for all targets.

    term::blank();
    term::info!(
        "{}/{} ({}) <- {}/{} ({})",
        term::format::dim(target_peer.id),
        term::format::highlight(&project.default_branch.to_string()),
        term::format::secondary(&term::format::oid(*target_oid)),
        term::format::dim(term::format::node(patches.public_key())),
        term::format::highlight(&head_branch.to_string()),
        term::format::secondary(&term::format::oid(head_oid)),
    );

    // TODO: Test case where the target branch has been re-written passed the merge-base, since the fork was created
    // This can also happen *after* the patch is created.

    term::patch::print_commits_ahead_behind(workdir, head_oid, (*target_oid).into())?;

    // List commits in patch that aren't in the target branch.
    term::blank();
    term::patch::list_commits(&commits)?;
    term::blank();

    if !term::confirm("Continue?") {
        anyhow::bail!("patch proposal aborted by user");
    }

    let message = head_commit
        .message()
        .ok_or(anyhow!("commit summary is not valid UTF-8; aborting"))?;
    let message = options.message.get(&format!("{}{}", message, PATCH_MSG));
    let (title, description) = message.split_once("\n\n").unwrap_or((&message, ""));
    let (title, description) = (title.trim(), description.trim());
    let description = description.replace(PATCH_MSG.trim(), ""); // Delete help message.

    if title.is_empty() {
        anyhow::bail!("a title must be given");
    }

    let title_pretty = &term::format::dim(format!("╭─ {} ───────", title));

    term::blank();
    term::print(title_pretty);
    term::blank();

    if description.is_empty() {
        term::print(term::format::italic("No description provided."));
    } else {
        term::markdown(&description);
    }

    term::blank();
    term::print(&term::format::dim(format!(
        "╰{}",
        "─".repeat(term::text_width(title_pretty) - 1)
    )));
    term::blank();

    if !term::confirm("Create patch?") {
        anyhow::bail!("patch proposal aborted by user");
    }

    let patch = patches.create(
        title,
        &description,
        MergeTarget::default(),
        base_oid,
        head_oid,
        &[],
        &signer,
    )?;

    term::blank();
    term::success!("Patch {} created 🌱", term::format::highlight(patch.id));

    if options.sync {
        // TODO
    }

    Ok(())
}

/// Update an existing patch with a new revision.
fn update<G: Signer>(
    mut patch: PatchMut,
    patch_id: PatchId,
    base: &Oid,
    head: &Oid,
    workdir: &git::raw::Repository,
    options: Options,
    signer: &G,
) -> anyhow::Result<()> {
    // TODO(cloudhead): Handle error.
    let (_, current_revision) = patch.latest().unwrap();
    let current_version = patch.version();

    if *current_revision.oid == *head {
        term::info!("Nothing to do, patch is already up to date.");
        return Ok(());
    }

    term::info!(
        "{} {} ({}) -> {} ({})",
        term::format::tertiary(term::format::cob(&patch_id)),
        term::format::dim(format!("R{}", current_version)),
        term::format::secondary(term::format::oid(current_revision.oid)),
        term::format::dim(format!("R{}", current_version + 1)),
        term::format::secondary(term::format::oid(*head)),
    );
    let message = options.message.get(REVISION_MSG);

    // Difference between the two revisions.
    term::patch::print_commits_ahead_behind(workdir, *head, *current_revision.oid)?;
    term::blank();

    if !term::confirm("Continue?") {
        anyhow::bail!("patch update aborted by user");
    }
    patch.update(message, *base, *head, signer)?;

    term::blank();
    term::success!("Patch {} updated 🌱", term::format::highlight(patch_id));
    term::blank();

    if options.sync {
        // TODO
    }

    Ok(())
}
