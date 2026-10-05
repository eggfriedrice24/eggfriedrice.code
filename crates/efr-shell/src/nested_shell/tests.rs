use proptest::prelude::*;

use super::starts_shell;

/// Lines that start an interactive shell, whose prompt must never be offered as a
/// question.
const SHELLS: &[&str] = &[
    "bash",
    "bash --norc --noprofile",
    "/usr/bin/zsh -l",
    "zsh -o vi",
    "sh",
    "dash -i < /dev/null",
    "fish",
    "ksh",
    "exec zsh",
    "LANG=C bash",
    "env -i TERM=dumb bash --norc",
    "nice -n 5 bash",
    "timeout 60 bash",
    "cd /tmp && bash",
    "pacman -Syu && bash",
    "(cd /tmp; zsh)",
    "{ bash; }",
    "for i in 1; do bash; done",
    "bash -c bash",
    "su",
    "su -",
    "su - root",
    "su -l postgres",
    "su -c 'bash -l' root",
    "sudo -i",
    "sudo -s",
    "sudo -u postgres -i",
    "sudo -iu postgres",
    "sudo --login",
    "sudo su -",
    "sudo bash",
    "sudo -E zsh",
    "sudo -i bash",
    "doas -s",
    "doas -u root bash",
    "ssh host",
    "ssh -p 2222 user@host",
    "ssh -o StrictHostKeyChecking=no host",
    "ssh host -l root",
    "ssh -t host sudo -i",
    "ssh -t host 'bash -l'",
    "script",
    "script -q /dev/null",
    "script -q -c sh /dev/null",
    "script --command=bash out.log",
    "script out.log -c bash",
    "eval bash",
    "echo $(bash)",
];

/// Lines that start no interactive shell, whose questions are offered.
const NOT_SHELLS: &[&str] = &[
    "pacman -Syu",
    "sudo pacman -Syu",
    "sudo -u postgres pg_dump app",
    "sudo -k pacman -Syu",
    "sudo -l",
    "sudo -v",
    "sudo -e /etc/hosts",
    "sudo -i pacman -Syu",
    "sudo -s 'apt upgrade'",
    "doas pacman -Syu",
    "sh -c 'printf \"ok? [Y/n] \"; read a'",
    "bash -lc make",
    "bash script.sh",
    "bash -- script.sh",
    "zsh --version",
    "fish --command='echo hi'",
    "curl -fsSL https://example.com/install.sh | sh",
    "curl -fsSL https://example.com/install.sh | sudo bash",
    "bash < script.sh",
    "bash <<< 'echo hi'",
    "su -c 'pacman -Syu'",
    "su root -c 'pacman -Syu'",
    "su --command='make install' root",
    "ssh host uptime",
    "ssh host bash",
    "ssh -N -L 8080:localhost:80 host",
    "ssh -T host",
    "ssh -f host",
    "echo hi | ssh host",
    r#"script -q -c "sh -c 'printf \"ok? [Y/n] \"; IFS= read -r a'" /dev/null"#,
    "script -c 'pacman -Syu' /dev/null",
    "script --version",
    "command -v bash",
    "echo bash",
    "printf '%s\\n' zsh",
    "which zsh # then bash",
    "eval 'sudo pacman -Syu'",
    "env",
    "timeout 5",
    "",
    "true 2>&1 >/dev/null",
];

/// Lines that leave a REPL or a shell in a container at its prompt.
const REPLS: &[&str] = &[
    "python",
    "python3",
    "python3.12 -q",
    "/usr/bin/python3 -X dev",
    "python3 -i script.py",
    "python3 -i -c 'import os'",
    "node",
    "nodejs",
    "node -i",
    "node --interactive",
    "irb",
    "irb -r json",
    "ghci",
    "ghci Main.hs",
    "ghci -fno-code Main.hs",
    "lua",
    "lua5.4",
    "lua -i script.lua",
    "lua -l socket",
    "psql",
    "psql -U postgres mydb",
    "psql -h localhost -d app",
    "PGPASSWORD=x psql -U me",
    "sudo -u postgres psql",
    "mysql",
    "mysql -u root -p",
    "mysql -uroot -psecret app",
    "mysql -h db --user=root",
    "mariadb -h db",
    "sqlite3",
    "sqlite3 app.db",
    "sqlite3 -header app.db",
    "sqlite3 -cmd '.tables' app.db",
    "docker exec -it web",
    "docker exec -it web bash",
    "docker exec -ti web sh -l",
    "docker exec -i -t -u root web /bin/bash",
    "docker exec --interactive --tty web zsh",
    "docker container exec -it web sh",
    "docker -H tcp://host:2375 exec -it web sh",
    "docker exec -it web python3",
    "docker exec -it -e TERM=xterm web bash",
    "podman exec -it db psql -U postgres",
    "kubectl exec -it pod",
    "kubectl exec -it pod -- bash",
    "kubectl -n prod exec -it pod -c app -- /bin/sh",
    "kubectl exec pod -it -- sh",
    "kubectl exec --stdin --tty deploy/web -- bash",
];

/// Lines that run a REPL's program or a container's command without leaving a prompt.
const NOT_REPLS: &[&str] = &[
    "python3 script.py",
    "python3 -c 'print(1)'",
    "python3 -m pip install requests",
    "python3 --version",
    "python3 -V",
    "echo 'print(1)' | python3",
    "python3 < script.py",
    "node app.js",
    "node -e 'console.log(1)'",
    "node -p 1+1",
    "node --version",
    "irb script.rb",
    "ghci -e main Main.hs",
    "lua script.lua",
    "lua -e 'print(1)'",
    "lua -v",
    "psql -c 'select 1'",
    "psql --command='select 1' app",
    "psql -f schema.sql app",
    "psql -l",
    "psql app < dump.sql",
    "mysql -e 'select 1'",
    "mysql --execute='select 1'",
    "mysql -psecret -e 'select 1'",
    "mysql app < dump.sql",
    "mysql --version",
    "sqlite3 app.db 'select 1'",
    "sqlite3 app.db < schema.sql",
    "sqlite3 -version",
    "docker ps",
    "docker exec web ls",
    "docker exec web bash",
    "docker exec -i web sh",
    "docker exec -it web ls -la",
    "docker exec -d web sh",
    "docker exec -it web sh -c 'apt upgrade'",
    "echo hi | docker exec -it web sh",
    "docker run -it ubuntu apt upgrade",
    "kubectl get pods",
    "kubectl exec pod -- ls",
    "kubectl exec -it pod -- ls",
    "kubectl exec -i pod -- sh",
];

#[test]
fn a_line_that_starts_an_interactive_shell_is_seen() {
    for line in SHELLS {
        assert!(starts_shell(line), "{line:?} starts a shell");
    }
}

#[test]
fn a_line_that_runs_a_command_is_no_shell() {
    for line in NOT_SHELLS {
        assert!(!starts_shell(line), "{line:?} starts no shell");
    }
}

#[test]
fn a_line_that_leaves_a_repl_or_a_shell_in_a_container_at_its_prompt_is_seen() {
    for line in REPLS {
        assert!(starts_shell(line), "{line:?} leaves a prompt");
    }
}

#[test]
fn a_line_that_runs_a_repls_program_or_a_containers_command_is_no_shell() {
    for line in NOT_REPLS {
        assert!(!starts_shell(line), "{line:?} leaves no prompt");
    }
}

#[test]
fn a_deep_line_inside_a_line_counts_as_no_shell() {
    assert!(starts_shell(&format!("{}bash", "eval ".repeat(3))));
    assert!(!starts_shell(&format!("{}bash", "eval ".repeat(100))));
    assert!(!starts_shell(&format!("{}bash", "sudo ".repeat(100))));
}

proptest! {
    #[test]
    fn any_line_is_read_without_a_panic(line in "\\PC{0,80}") {
        starts_shell(&line);
    }

    #[test]
    fn any_line_of_shell_syntax_is_read_without_a_panic(
        line in "[a-z -]{0,10}[\"'$\\\\|&;()<>`#=\\n -]{0,20}[a-z -]{0,10}",
    ) {
        starts_shell(&line);
    }
}
