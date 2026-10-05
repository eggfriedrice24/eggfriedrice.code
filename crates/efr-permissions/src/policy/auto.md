| 83 | `rm` |  |  |  |  |  |  |  |  |
| 84 | `rmdir` |  |  |  |  |  |  |  |  |
| 85 | `mkdir` |  |  |  |  |  |  |  |  |
| 86 | `touch` |  |  |  |  |  |  |  |  |
| 87 | `mv` |  |  |  |  |  |  |  |  |
| 88 | `cp` |  |  |  |  |  |  |  |  |
| 89 | `ln` |  |  |  |  |  |  |  |  |
| 90 | `chmod` |  |  |  |  |  |  |  |  |
| 91 | `truncate` |  |  |  |  |  |  |  |  |
| 92 | `tee` |  |  |  |  |  |  |  |  |
| 93-96 | `cargo` | `build\|check\|test\|fetch\|update` | `--manifest-path` `--config` `--target-dir` `-Z` `-C` |  |  |  |  | project, scratch | yes |
| 97-98 | `cargo` | `clippy\|fmt\|doc\|run\|bench\|nextest\|tree\|metadata` | `--manifest-path` `--config` `--target-dir` `-Z` `-C` `--open` |  |  |  |  | project, scratch |  |
| 99-100 | `just` |  | `-f` `--justfile` `-d` `--working-directory` `-g` `--global-justfile` `--set` `-c` `--command` `--shell*` `--dotenv-path` `=` |  |  |  |  | project, scratch |  |
| 101-102 | `make` |  | `-C` `--directory` `-f` `--file` `--makefile` `-I` `--include-dir` `--eval` `-E` `=` |  |  |  |  | project, scratch |  |
| 103-106 | `npm` | `install\|ci` | `--script-shell` `--prefix` `--dir` `--cwd` `-C` `-g` `--global` `--location` `--registry` `--userconfig` |  | 0 |  |  | project, scratch | yes |
| 107-108 | `npm` | `run\|test\|build\|lint` | `--script-shell` `--prefix` `--dir` `--cwd` `-C` `-g` `--global` `--location` `--registry` `--userconfig` |  |  |  |  | project, scratch |  |
| 109-112 | `pnpm` | `install\|ci` | `--script-shell` `--prefix` `--dir` `--cwd` `-C` `-g` `--global` `--location` `--registry` `--userconfig` |  | 0 |  |  | project, scratch | yes |
| 113-114 | `pnpm` | `run\|test\|build\|lint` | `--script-shell` `--prefix` `--dir` `--cwd` `-C` `-g` `--global` `--location` `--registry` `--userconfig` |  |  |  |  | project, scratch |  |
| 115-118 | `yarn` | `install\|ci` | `--script-shell` `--prefix` `--dir` `--cwd` `-C` `-g` `--global` `--location` `--registry` `--userconfig` |  | 0 |  |  | project, scratch | yes |
| 119-120 | `yarn` | `run\|test\|build\|lint` | `--script-shell` `--prefix` `--dir` `--cwd` `-C` `-g` `--global` `--location` `--registry` `--userconfig` |  |  |  |  | project, scratch |  |
| 121-124 | `bun` | `install\|ci` | `--script-shell` `--prefix` `--dir` `--cwd` `-C` `-g` `--global` `--location` `--registry` `--userconfig` |  | 0 |  |  | project, scratch | yes |
| 125-126 | `bun` | `run\|test\|build\|lint` | `--script-shell` `--prefix` `--dir` `--cwd` `-C` `-g` `--global` `--location` `--registry` `--userconfig` |  |  |  |  | project, scratch |  |
| 127-128 | `go` | `build\|test\|vet\|fmt\|run` | `-C*` `-modfile*` `-overlay*` `-toolexec*` `-exec*` `@` |  |  |  |  | project, scratch |  |
| 129-130 | `go` | `mod` `tidy` | `-C*` `-modfile*` `-overlay*` `-toolexec*` `-exec*` `@` |  |  |  |  | project, scratch |  |
| 131-134 | `go` | `mod` `download` | `-C*` `-modfile*` `-overlay*` `-toolexec*` `-exec*` `@` |  | 0 |  |  | project, scratch | yes |
| 135-136 | `pytest` |  | `--rootdir` `-c` `--config-file` `-p` |  |  |  |  | project, scratch |  |
| 137-138 | `uv` | `run` | `--directory` `--project` `--with*` `--index*` `--extra-index-url` `--default-index` `--find-links` |  |  |  |  | project, scratch |  |
| 139-142 | `uv` | `sync` | `--directory` `--project` `--with*` `--index*` `--extra-index-url` `--default-index` `--find-links` |  | 0 |  |  | project, scratch | yes |
| 143-144 | `ruff` |  |  |  |  |  |  | project, scratch |  |
| 145-146 | `mypy` |  |  |  |  |  |  | project, scratch |  |
| 147-148 | `rustfmt` |  |  |  |  |  |  | project, scratch |  |
| 149-150 | `prettier` |  |  |  |  |  |  | project, scratch |  |
| 151-152 | `eslint` |  |  |  |  |  |  | project, scratch |  |
| 153-154 | `tsc` |  |  |  |  |  |  | project, scratch |  |
| 155-156 | `zig` | `build` | `-p` `--prefix*` `--build-file` |  |  |  |  | project, scratch |  |
| 157-158 | `git` | `add` |  |  |  |  |  | project, scratch |  |
| 159-160 | `git` | `commit` |  |  |  |  |  | project, scratch |  |
| 161-162 | `git` | `switch` | `--discard-changes` `-f` `--force` |  |  |  |  | project, scratch |  |
| 163-164 | `git` | `checkout` | `-p` `--patch` `-f` `--force` `--ours` `--theirs` `-m` `--merge` `--conflict` `--overlay` `--no-overlay` `--pathspec-from-file` |  | 2 |  | `ref_names` | project, scratch |  |
| 165-166 | `git` | `restore` `--staged\|-S` | `-W` `--worktree` `-p` `--patch` |  |  |  |  | project, scratch |  |
| 167-168 | `git` | `stash` |  |  | 0 | 0 |  | project, scratch |  |
| 169-170 | `git` | `stash` `push\|list\|show\|apply\|pop` |  |  |  |  |  | project, scratch |  |
| 171-172 | `git` | `merge` | `-s` `--strategy` |  |  |  |  | project, scratch |  |
| 173-174 | `git` | `rebase` | `-i` `--interactive` `-x` `--exec` `--edit-todo` `-s` `--strategy` |  |  |  |  | project, scratch |  |
| 175-176 | `git` | `cherry-pick` | `-s` `--strategy` |  |  |  |  | project, scratch |  |
| 177-178 | `git` | `tag` | `-d` `--delete` `-f` `--force` `-s` `--sign` `-u` `--local-user` `-v` `--verify` |  |  |  |  | project, scratch |  |
| 179-180 | `git` | `mv` |  |  |  |  |  | project, scratch |  |
| 181-182 | `git` | `rm` |  |  |  |  |  | project, scratch |  |
| 183-184 | `git` | `worktree` `add\|list` |  |  |  |  |  | project, scratch |  |
| 185-188 | `git` | `fetch` | `--upload-pack` `--exec` `-o` `--server-option` `-s` `--strategy` |  |  |  | `ref_names` | project, scratch | yes |
| 189-192 | `git` | `pull` | `--upload-pack` `--exec` `-o` `--server-option` `-s` `--strategy` |  |  |  | `ref_names` | project, scratch | yes |
