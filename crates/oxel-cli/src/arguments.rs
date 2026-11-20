//! Declares the command surface and renders CLI usage diagnostics.

use std::{
    env,
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    process::ExitCode,
};

use oxel::{Diagnostic, Span};
use usage_rs::{Args, Cli, Error, Subcommands};

use crate::diagnostics::Format;

/// Compile and run CLR-native oxel programs.
#[derive(Cli)]
#[usage(bin = "oxel", version, unknown_flags = "error")]
struct CommandLine {
    /// Read project inputs from this oxel.toml file.
    #[usage(long, global)]
    manifest_path: Option<PathBuf>,
    /// Resolve .NET framework references from the selected SDK.
    #[usage(long, global)]
    framework: bool,
    /// Require an existing lock file for NuGet dependencies.
    #[usage(long, global)]
    locked: bool,
    /// Format compiler diagnostics.
    #[usage(long, global, value_enum, default = "text")]
    diagnostic_format: Format,
    #[usage(subcommand)]
    command: Command,
}

#[derive(Subcommands)]
enum Command {
    /// Generate a managed .NET assembly.
    Compile(BuildInputs),
    /// Check syntax and types.
    Check(CheckInputs),
    /// Compile and run on CoreCLR.
    Run(RunInputs),
    /// Compile and run Oxel unit tests on CoreCLR.
    Test(TestInputs),
    /// Publish a native executable with the .NET SDK.
    Publish(BuildInputs),
    /// Restore SDK dependencies and write the NuGet lock file.
    Restore(RestoreInputs),
    /// Start the language server over stdio.
    Lsp(EditorInputs),
}

#[derive(Args, Default)]
struct BuildInputs {
    /// Include a local CLR assembly.
    #[usage(long)]
    reference: Vec<PathBuf>,
    /// Source files. When omitted, read oxel.toml in the working directory.
    sources: Vec<PathBuf>,
}

#[derive(Args)]
struct CheckInputs {
    #[usage(flatten)]
    build: BuildInputs,
    /// Check the unit test module.
    #[usage(long)]
    tests: bool,
}

#[derive(Args)]
struct RunInputs {
    #[usage(flatten)]
    build: BuildInputs,
    /// Arguments passed to the program after --.
    #[usage(double_dash = "required")]
    arguments: Vec<OsString>,
}

#[derive(Args)]
struct TestInputs {
    #[usage(flatten)]
    build: BuildInputs,
    /// Select tests whose names contain this case-sensitive text.
    #[usage(long)]
    filter: Option<String>,
    /// Run matching tests marked with #[ignore] as well.
    #[usage(long)]
    include_ignored: bool,
    /// List matching tests without running them.
    #[usage(long)]
    list: bool,
}

#[derive(Args)]
struct RestoreInputs {
    /// Restore the Native AOT graph.
    #[usage(long)]
    native: bool,
}

#[derive(Args)]
struct EditorInputs {
    /// Include a local CLR assembly.
    #[usage(long)]
    reference: Vec<PathBuf>,
    /// Include a source file in the editor's module.
    #[usage(long)]
    source: Vec<PathBuf>,
    /// Include the manifest's test source files in the editor module.
    #[usage(long)]
    tests: bool,
}

pub(super) enum Action {
    Compile,
    Check {
        tests: bool,
    },
    Run,
    Test {
        filter: Option<String>,
        include_ignored: bool,
        list: bool,
    },
    Publish,
    Restore,
    Lsp {
        tests: bool,
    },
}

pub(super) struct Options {
    pub action: Action,
    pub format: Format,
    pub framework: bool,
    pub locked: bool,
    pub manifest_path: Option<PathBuf>,
    pub native: bool,
    pub sources: Vec<PathBuf>,
    pub references: Vec<PathBuf>,
    pub arguments: Vec<OsString>,
}

pub(super) fn parse() -> Result<Options, ExitCode> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    let argv: Vec<_> = args.iter().map(OsString::as_os_str).collect();
    if usage_rs::is_spec_request(CommandLine::command(), &argv) {
        print!("{}", CommandLine::to_kdl());
        return Err(ExitCode::SUCCESS);
    }
    let parsed = match CommandLine::parse_from(&argv) {
        Ok(parsed) => parsed,
        Err(Error::Help { cmd, long }) => {
            print!(
                "{}",
                CommandLine::render_help(cmd, long).unwrap_or_default()
            );
            return Err(ExitCode::SUCCESS);
        }
        Err(Error::Version { .. }) => {
            println!("oxel {}", env!("CARGO_PKG_VERSION"));
            return Err(ExitCode::SUCCESS);
        }
        Err(error) => {
            let message = usage_rs::render_failure_plain(CommandLine::spec(), &argv, &error);
            let message = message.strip_prefix("error: ").unwrap_or(&message);
            return Err(usage_error(requested_format(&argv), message.trim_end()));
        }
    };
    let format = parsed.diagnostic_format;
    if !matches!(parsed.command, Command::Run(_)) && argv.contains(&OsStr::new("--")) {
        return Err(usage_error(format, "only oxel run accepts --"));
    }
    let mut native = false;
    let mut arguments = Vec::new();
    let (action, build) = match parsed.command {
        Command::Compile(build) => (Action::Compile, build),
        Command::Check(check) => (Action::Check { tests: check.tests }, check.build),
        Command::Run(run) => {
            arguments = run.arguments;
            (Action::Run, run.build)
        }
        Command::Test(test) => (
            Action::Test {
                filter: test.filter,
                include_ignored: test.include_ignored,
                list: test.list,
            },
            test.build,
        ),
        Command::Publish(build) => {
            native = true;
            (Action::Publish, build)
        }
        Command::Restore(restore) => {
            native = restore.native;
            (Action::Restore, BuildInputs::default())
        }
        Command::Lsp(editor) => (
            Action::Lsp {
                tests: editor.tests,
            },
            BuildInputs {
                sources: editor.source,
                reference: editor.reference,
            },
        ),
    };
    Ok(Options {
        action,
        format,
        framework: parsed.framework,
        locked: parsed.locked,
        manifest_path: parsed.manifest_path,
        native,
        sources: build.sources,
        references: build.reference,
        arguments,
    })
}

pub(super) fn usage_error(format: Format, message: &str) -> ExitCode {
    format.print(
        Path::new("<command line>"),
        "",
        &Diagnostic::new("E0001", Span::default(), message),
    );
    ExitCode::from(2)
}

// A parse error has no typed options. Respect the requested diagnostic format
// even then, stopping at the delimiter before application arguments.
fn requested_format(argv: &[&OsStr]) -> Format {
    let end = argv
        .iter()
        .position(|arg| *arg == "--")
        .unwrap_or(argv.len());
    let mut args = argv[..end].iter().filter_map(|arg| arg.to_str());
    let mut format = Format::default();
    while let Some(arg) = args.next() {
        let value = if arg == "--diagnostic-format" {
            args.next()
        } else {
            arg.strip_prefix("--diagnostic-format=")
        };
        if let Some(requested) = value.and_then(<Format as usage_rs::spec::ValueEnum>::from_choice)
        {
            format = requested;
        }
    }
    format
}
