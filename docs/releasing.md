# Releasing

This file tells how to make a release of efr. A release is a tag `v<version>` on
`main`. Two workflows do the work:

- `.github/workflows/release.yml` starts on the tag. It builds `efrd`, `efr` and
  `efr-sbx` as `just build-release` does, packs
  `efr-code-<version>-x86_64-linux.tar.gz`
  with a `.sha256` file, and makes the GitHub release. Then it starts `aur.yml`.
- `.github/workflows/aur.yml` publishes two AUR packages from `packaging/`:
  `efr-code` builds from the tag tarball, and `efr-code-bin` installs the release
  tarball.
  It builds and checks both packages before it pushes either.

## One-time setup

Do these steps once, before the first release.

1. Make sure that the AUR names are free. This request must return
   `"resultcount":0`, or show packages of the account `eggfriedrice`:

   ```sh
   curl -s 'https://aur.archlinux.org/rpc/v5/info?arg[]=efr-code&arg[]=efr-code-bin'
   ```

2. Give the AUR account `eggfriedrice` an SSH key for the workflow. You can use the
   key of frameit again. For a new key, run
   `ssh-keygen -t ed25519 -f aur -C efr-ci` with no passphrase, and add `aur.pub`
   under My Account, SSH Public Key, on <https://aur.archlinux.org/>.
3. Put the private key in the repository secret `AUR_SSH_PRIVATE_KEY`:

   ```sh
   gh secret set AUR_SSH_PRIVATE_KEY < aur
   ```

The AUR makes the package repositories `efr-code` and `efr-code-bin` on the first
push.

## Why the workflows never write to main

A ruleset protects `main`. A change needs a pull request and the status checks.
Only admins can bypass these rules, and the token of a workflow is not an admin. So
`aur.yml` cannot commit to `main`. Instead, the release commit sets the version in
all places before the tag, and the workflows only check it.

The sums that change with each release are not in `packaging/`. The PKGBUILDs keep
`SKIP` for them. `aur.yml` fills them in its build copy, compares them with the
sums from upstream, and pushes that copy to the AUR. The other sums are pinned: the
ghostty archive and the Zig tarball of `packaging/efr-code/PKGBUILD`. `aur.yml` stops
when one of them changes.

## Make a release

1. Start from a clean `main` that has passed CI.
2. Set the new version in these places, in one commit:
   - `CHANGELOG.md`: rename the `[Unreleased]` section to `[<version>] - <date>`,
     put a new empty `[Unreleased]` section above it, and update the compare links
     at the bottom. `release.yml` copies this section into the release notes, and it
     stops when the section is missing or empty;
   - `version` in `[workspace.package]` of `Cargo.toml`;
   - `Cargo.lock` (run `cargo metadata --format-version 1 > /dev/null`);
   - `pkgver` in `packaging/efr-code/PKGBUILD` and
     `packaging/efr-code-bin/PKGBUILD`. Keep `pkgrel=1`.

   Then run the gates of `AGENTS.md`, and commit with a message such as
   `release 0.0.2`.
3. Push the commit and wait until CI on `main` is green.
4. Make the tag on that commit and push it. Only admins can make a `v*` tag.

   ```sh
   git tag -a v0.0.2 -m 'efr 0.0.2'
   git push origin v0.0.2
   ```

5. Watch `release.yml`. It stops before it publishes if the tag, `Cargo.toml` or a
   `pkgver` do not agree, or if `CHANGELOG.md` has no section for the version.

   ```sh
   gh run watch "$(gh run list --workflow release.yml --limit 1 --json databaseId --jq '.[0].databaseId')"
   ```

6. Watch `aur.yml` in the same way. It builds `efr-code` from source and runs its
   `check()`, so it can take 20 minutes or more.
7. Check the packages on <https://aur.archlinux.org/packages/efr-code> and
   <https://aur.archlinux.org/packages/efr-code-bin>.

## When something fails

- A `v*` tag cannot move and cannot be deleted. If the commit of the tag is wrong,
  make a new version.
- If `release.yml` fails before the release exists, fix the cause on `main`, then
  run the workflow again on the same tag. It uses the code of the tag, so a fix of
  the code needs a new version.
- If `aur.yml` fails, nothing is pushed to the AUR. Run it again on the same tag:

  ```sh
  gh workflow run aur.yml --ref main -f tag=v0.0.1
  ```

  A package that the AUR already has at this version is skipped.
- A change of the packaging only, with the same efr version, needs a higher
  `pkgrel`. `aur.yml` does not do this; push it to the AUR by hand.

## Test a package before a release

Build the packages in a container, as `aur.yml` does, and never on your own system:
use the `archlinux:base-devel` image of Docker. For `efr-code`, put a tarball of the tree
from `git archive --prefix=eggfriedrice.code-<version>/` next to the PKGBUILD as
`efr-code-<version>.tar.gz`; makepkg then uses that file and does not download the tag.
For `efr-code-bin`, make the release tarball with the commands of `release.yml` and put it
next to that PKGBUILD.

## Keep the changelog current

Add each change that a user can notice to the `[Unreleased]` section of
`CHANGELOG.md`, in the commit that makes it or soon after. Use the groups of
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/): Added, Changed, Deprecated,
Removed, Fixed and Security. Write for users: say what changes for them, not which
function changed.
