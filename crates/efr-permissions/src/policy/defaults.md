| 8 | `ls` |  |  |  |
| 9 | `pwd` |  |  |  |
| 10 | `cat` |  |  |  |
| 11 | `head` |  |  |  |
| 12 | `tail` |  | `-f` `-F` `--follow` |  |
| 13 | `wc` |  |  |  |
| 14 | `file` |  | `-C` `--compile` |  |
| 15 | `stat` |  |  |  |
| 16 | `du` |  |  |  |
| 17 | `df` |  |  |  |
| 18 | `lsblk` |  |  |  |
| 19 | `blkid` |  | `-g` `--garbage-collect` `-c` `--cache-file` |  |
| 20 | `findmnt` |  | `-p` `--poll` |  |
| 21 | `free` |  | `-s` `--seconds` |  |
| 22 | `uptime` |  |  |  |
| 23 | `uname` |  |  |  |
| 24 | `whoami` |  |  |  |
| 25 | `id` |  |  |  |
| 26 | `groups` |  |  |  |
| 27 | `hostname` |  | `-F` `--file` `-b` `--boot` | 0 |
| 28 | `date` |  | `-s` `--set` |  |
| 29 | `which` |  |  |  |
| 30 | `type` |  |  |  |
| 31 | `command` | `-v\|-V` |  |  |
| 32 | `echo` |  |  |  |
| 33 | `printf` |  | `-v` |  |
| 34 | `realpath` |  |  |  |
| 35 | `readlink` |  |  |  |
| 36 | `basename` |  |  |  |
| 37 | `dirname` |  |  |  |
| 38 | `tree` |  | `-o` `-R` |  |
| 39 | `rg` |  | `--pre` `--hostname-bin` |  |
| 40 | `grep` |  |  |  |
| 41 | `egrep` |  |  |  |
| 42 | `fgrep` |  |  |  |
| 43 | `diff` |  |  |  |
| 44 | `cmp` |  |  |  |
| 45 | `sort` |  | `-o` `--output` `--compress-program` |  |
| 46 | `uniq` |  |  | 1 |
| 47 | `cut` |  |  |  |
| 48 | `tr` |  |  |  |
| 49 | `column` |  |  |  |
| 50 | `jq` |  | `-i` `--in-place` `env` `ENV` |  |
| 51 | `ps` |  | `e` |  |
| 52 | `pgrep` |  |  |  |
| 53 | `ss` |  | `-K` `--kill` `-D` `--diag` |  |
| 54 | `ip` | `addr\|address\|a` |  | 0 |
| 55 | `ip` | `addr\|address\|a` `show\|list` |  |  |
| 56 | `ip` | `route\|r` |  | 0 |
| 57 | `ip` | `route\|r` `show\|list` |  |  |
| 58 | `ip` | `link\|l` |  | 0 |
| 59 | `ip` | `link\|l` `show\|list` |  |  |
| 60 | `journalctl` |  | `--vacuum*` `--rotate` `--flush` `--sync` `--relinquish-var` `--smart-relinquish-var` `--setup-keys` `--update-catalog` `--cursor-file` `-f` `--follow` |  |
| 61 | `systemctl` | `status\|list-units\|list-unit-files\|is-active\|is-enabled\|is-failed\|show\|cat` | `-H` `--host` |  |
| 62 | `systemctl` | `--user` `status\|list-units\|list-unit-files\|is-active\|is-enabled\|is-failed\|show\|cat` | `-H` `--host` |  |
| 63 | `git` | `status\|diff\|log\|show\|rev-parse\|ls-files\|blame` | `--output` |  |
| 64 | `git` | `--no-pager` `status\|diff\|log\|show\|rev-parse\|ls-files\|blame` | `--output` |  |
| 65 | `git` | `branch` | `-d` `-D` `--delete` `-m` `-M` `--move` `-c` `-C` `--copy` `-f` `--force` `-u` `--set-upstream-to` `--unset-upstream` `--edit-description` `-t` `--track` `--no-track` `--create-reflog` | 0 |
| 66 | `git` | `--no-pager` `branch` | `-d` `-D` `--delete` `-m` `-M` `--move` `-c` `-C` `--copy` `-f` `--force` `-u` `--set-upstream-to` `--unset-upstream` `--edit-description` `-t` `--track` `--no-track` `--create-reflog` | 0 |
| 67 | `git` | `remote` |  | 0 |
| 68 | `git` | `--no-pager` `remote` |  | 0 |
| 69 | `pacman` | `-Q*\|--query` |  |  |
| 70 | `pacman` | `-Ss\|-Ssq\|-Sqs\|-Si\|-Sii` | `-y` `-u` `-w` `-c` `--refresh` `--sysupgrade` `--downloadonly` `--clean` |  |
| 71 | `lspci` |  |  |  |
| 72 | `lsusb` |  |  |  |
| 73 | `sensors` |  | `-s` `--set` |  |
| 74 | `nproc` |  |  |  |
| 75 | `find` |  | `-exec` `-execdir` `-ok` `-okdir` `-delete` `-fprint*` `-fls` |  |
