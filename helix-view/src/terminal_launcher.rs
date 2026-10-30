//! Opening the terminal a debuggee runs in (the DAP `runInTerminal` reverse request).
//!
//! A [`Launcher`] knows how to ask one terminal or multiplexer for a new pane or window with a
//! given working directory, title and command. [`Launcher::detect`] picks the one Helix is
//! running in from the environment variables those programs export to their children.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// A terminal or multiplexer that can open a pane or window from the command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Launcher {
    Tmux,
    Zellij,
    Wezterm,
    Kitty,
    Alacritty,
}

/// What to run, and where.
#[derive(Debug, Clone, Copy)]
pub struct Launch<'a> {
    pub cwd: &'a str,
    pub title: Option<&'a str>,
    /// The debuggee's command line.
    pub args: &'a [String],
    /// Environment variables of the debuggee; `None` unsets one.
    pub env: Option<&'a HashMap<String, Option<String>>>,
}

impl Launcher {
    /// In the order [`Launcher::detect`] tries them. Multiplexers come before terminal
    /// emulators because they run inside one: when `TMUX` and `KITTY_WINDOW_ID` are both set,
    /// the pane belongs in tmux. (Nesting two multiplexers cannot be told apart from the
    /// environment; configure `launcher` then.)
    const DETECTION_ORDER: [Launcher; 5] = [
        Launcher::Tmux,
        Launcher::Zellij,
        Launcher::Wezterm,
        Launcher::Kitty,
        Launcher::Alacritty,
    ];

    /// The program that is run to ask for the pane.
    pub fn program(self) -> &'static str {
        match self {
            Launcher::Tmux => "tmux",
            Launcher::Zellij => "zellij",
            Launcher::Wezterm => "wezterm",
            Launcher::Kitty => "kitty",
            Launcher::Alacritty => "alacritty",
        }
    }

    /// The environment variable that is set in the programs run inside this terminal.
    fn marker_env(self) -> &'static str {
        match self {
            Launcher::Tmux => "TMUX",
            Launcher::Zellij => "ZELLIJ",
            Launcher::Wezterm => "WEZTERM_UNIX_SOCKET",
            Launcher::Kitty => "KITTY_WINDOW_ID",
            // Only set when the IPC socket `alacritty msg` talks to exists.
            Launcher::Alacritty => "ALACRITTY_SOCKET",
        }
    }

    /// The launcher of the terminal we are running in: its marker variable is set and its
    /// program is installed.
    pub fn detect(is_set: impl Fn(&str) -> bool, exists: impl Fn(&str) -> bool) -> Option<Self> {
        Self::DETECTION_ORDER
            .into_iter()
            .find(|launcher| is_set(launcher.marker_env()) && exists(launcher.program()))
    }

    /// The command that opens the pane. The command returns once the pane exists, it does not
    /// stay around for as long as the debuggee does.
    pub fn command_line(self, launch: &Launch) -> (String, Vec<String>) {
        fn push(args: &mut Vec<String>, parts: &[&str]) {
            args.extend(parts.iter().map(|part| part.to_string()));
        }
        let mut args: Vec<String> = Vec::new();

        match self {
            Launcher::Tmux => {
                push(&mut args, &["split-window", "-c", launch.cwd]);
            }
            Launcher::Zellij => {
                push(&mut args, &["run", "--cwd", launch.cwd]);
                if let Some(title) = launch.title {
                    push(&mut args, &["--name", title]);
                }
                push(&mut args, &["--"]);
            }
            Launcher::Wezterm => {
                push(&mut args, &["cli", "split-pane", "--cwd", launch.cwd, "--"]);
            }
            Launcher::Kitty => {
                push(&mut args, &["@", "launch", "--type=window"]);
                args.push(format!("--cwd={}", launch.cwd));
                if let Some(title) = launch.title {
                    args.push(format!("--title={title}"));
                }
                push(&mut args, &["--"]);
            }
            Launcher::Alacritty => {
                push(
                    &mut args,
                    &["msg", "create-window", "--working-directory", launch.cwd],
                );
                if let Some(title) = launch.title {
                    push(&mut args, &["--title", title]);
                }
                push(&mut args, &["-e"]);
            }
        }
        args.extend(with_env(launch.args, launch.env));

        (self.program().to_string(), args)
    }
}

/// Puts the environment in front of the command, with `env(1)`: the pane is opened by a
/// server (tmux, zellij, ...) that doesn't inherit the environment of the process asking for it.
pub fn with_env(args: &[String], env: Option<&HashMap<String, Option<String>>>) -> Vec<String> {
    let Some(env) = env.filter(|env| !env.is_empty()) else {
        return args.to_vec();
    };

    // Sorted, so the command line doesn't change from one run to the next.
    let mut entries: Vec<_> = env.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));

    let mut line = vec!["env".to_string()];
    for (name, value) in entries {
        match value {
            Some(value) => line.push(format!("{name}={value}")),
            None => {
                line.push("-u".to_string());
                line.push(name.clone());
            }
        }
    }
    line.extend(args.iter().cloned());
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(vars: &'static [&'static str]) -> impl Fn(&str) -> bool {
        move |name| vars.contains(&name)
    }

    fn installed(programs: &'static [&'static str]) -> impl Fn(&str) -> bool {
        move |name| programs.contains(&name)
    }

    fn debuggee() -> Vec<String> {
        vec!["./target/debug/app".into(), "--flag".into()]
    }

    fn launch<'a>(
        args: &'a [String],
        env: Option<&'a HashMap<String, Option<String>>>,
    ) -> Launch<'a> {
        Launch {
            cwd: "/work/app",
            title: Some("Debug"),
            args,
            env,
        }
    }

    #[test]
    fn detects_the_terminal_by_its_environment() {
        let all = installed(&["tmux", "zellij", "wezterm", "kitty", "alacritty"]);
        assert_eq!(
            Launcher::detect(set(&["KITTY_WINDOW_ID"]), &all),
            Some(Launcher::Kitty)
        );
        assert_eq!(
            Launcher::detect(set(&["ALACRITTY_SOCKET"]), &all),
            Some(Launcher::Alacritty)
        );
        assert_eq!(
            Launcher::detect(set(&["ZELLIJ"]), &all),
            Some(Launcher::Zellij)
        );
        assert_eq!(Launcher::detect(set(&[]), &all), None);
    }

    #[test]
    fn a_multiplexer_wins_over_the_terminal_it_runs_in() {
        let all = installed(&["tmux", "zellij", "wezterm", "kitty", "alacritty"]);
        assert_eq!(
            Launcher::detect(set(&["KITTY_WINDOW_ID", "TMUX"]), &all),
            Some(Launcher::Tmux)
        );
        assert_eq!(
            Launcher::detect(set(&["ALACRITTY_SOCKET", "ZELLIJ"]), &all),
            Some(Launcher::Zellij)
        );
    }

    #[test]
    fn needs_the_program_to_be_installed() {
        assert_eq!(
            Launcher::detect(set(&["KITTY_WINDOW_ID"]), installed(&[])),
            None
        );
        // the variable of a terminal that cannot be driven doesn't hide one that can
        assert_eq!(
            Launcher::detect(set(&["TMUX", "KITTY_WINDOW_ID"]), installed(&["kitty"])),
            Some(Launcher::Kitty)
        );
    }

    #[test]
    fn kitty_command_line() {
        let args = debuggee();
        let (program, line) = Launcher::Kitty.command_line(&launch(&args, None));
        assert_eq!(program, "kitty");
        assert_eq!(
            line,
            [
                "@",
                "launch",
                "--type=window",
                "--cwd=/work/app",
                "--title=Debug",
                "--",
                "./target/debug/app",
                "--flag"
            ]
        );
    }

    #[test]
    fn alacritty_command_line() {
        let args = debuggee();
        let (program, line) = Launcher::Alacritty.command_line(&launch(&args, None));
        assert_eq!(program, "alacritty");
        assert_eq!(
            line,
            [
                "msg",
                "create-window",
                "--working-directory",
                "/work/app",
                "--title",
                "Debug",
                "-e",
                "./target/debug/app",
                "--flag"
            ]
        );
    }

    #[test]
    fn zellij_command_line() {
        let args = debuggee();
        let (program, line) = Launcher::Zellij.command_line(&launch(&args, None));
        assert_eq!(program, "zellij");
        assert_eq!(
            line,
            [
                "run",
                "--cwd",
                "/work/app",
                "--name",
                "Debug",
                "--",
                "./target/debug/app",
                "--flag"
            ]
        );
    }

    #[test]
    fn tmux_and_wezterm_get_the_working_directory() {
        let args = debuggee();
        let none = Launch {
            title: None,
            ..launch(&args, None)
        };
        let (_, tmux) = Launcher::Tmux.command_line(&none);
        assert_eq!(
            tmux,
            [
                "split-window",
                "-c",
                "/work/app",
                "./target/debug/app",
                "--flag"
            ]
        );
        let (_, wezterm) = Launcher::Wezterm.command_line(&none);
        assert_eq!(
            wezterm,
            [
                "cli",
                "split-pane",
                "--cwd",
                "/work/app",
                "--",
                "./target/debug/app",
                "--flag"
            ]
        );
    }

    #[test]
    fn the_environment_goes_through_env() {
        let args = debuggee();
        let env = HashMap::from([
            ("RUST_LOG".to_string(), Some("debug".to_string())),
            ("COLUMNS".to_string(), None),
        ]);
        let (_, line) = Launcher::Zellij.command_line(&launch(&args, Some(&env)));
        assert_eq!(
            &line[5..],
            [
                "--",
                "env",
                "-u",
                "COLUMNS",
                "RUST_LOG=debug",
                "./target/debug/app",
                "--flag"
            ]
        );
        // nothing to set: the command is left alone
        assert_eq!(with_env(&args, Some(&HashMap::new())), args);
    }

    #[test]
    fn launcher_names_in_the_config() {
        #[derive(Deserialize)]
        struct Wrapper {
            launcher: Launcher,
        }
        let parsed: Wrapper = toml::from_str(r#"launcher = "kitty""#).unwrap();
        assert_eq!(parsed.launcher, Launcher::Kitty);
        assert!(toml::from_str::<Wrapper>(r#"launcher = "xterm""#).is_err());
    }

    #[test]
    fn terminal_table_in_the_config() {
        use crate::editor::TerminalConfig;

        let config: TerminalConfig = toml::from_str(r#"launcher = "zellij""#).unwrap();
        assert_eq!(config, TerminalConfig::with_launcher(Launcher::Zellij));

        let config: TerminalConfig = toml::from_str("command = \"foot\"\nargs = [\"-e\"]").unwrap();
        assert_eq!(config.launcher, None);
        assert_eq!(config.command, "foot");
    }
}
