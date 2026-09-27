//! `lumenc`, the Lumen command-line driver: argument parsing and I/O around
//! the [`lumen`] library. `lumenc --help` lists the commands.
//!
//! Logging goes to stderr and is set by `RUST_LOG`, e.g. `RUST_LOG=lumen=debug`.

use std::process::ExitCode;

use lumen::backend::{Program, disassemble, execute};
use lumen::hir::print_hir;
use lumen::opt::OptOptions;
use lumen::parser::print::print_ast;
use lumen::session::{PipelineOptions, Session, Stage};

fn main() -> ExitCode {
    init_tracing();
    match Cli::parse(std::env::args().skip(1)) {
        Ok(cli) => run(cli),
        Err(usage) => {
            eprintln!("{usage}\n");
            eprint!("{HELP}");
            ExitCode::from(2)
        }
    }
}

/// Logs to stderr, filtered by `RUST_LOG` (default `warn`).
fn init_tracing() {
    use tracing_subscriber::{EnvFilter, fmt};
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    // Tolerate a subscriber that is already installed.
    let _ = fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

#[derive(Debug)]
enum Command {
    Run,
    Check,
    Fmt,
    Dump(Stage),
    /// Writes an object file to `--output`, or stdout.
    Build,
    /// Runs an object file.
    Exec,
    /// Explains a diagnostic code.
    Explain(String),
}

#[derive(Debug)]
struct Cli {
    command: Command,
    path: String,
    output: Option<String>,
    optimize: bool,
    show_timings: bool,
}

impl Cli {
    /// Parses the arguments after the program name. `Err` holds the message
    /// to print above the usage text.
    fn parse(args: impl Iterator<Item = String>) -> Result<Cli, String> {
        let mut positional = Vec::new();
        let mut optimize = true;
        let mut show_timings = false;
        let mut output = None;
        let mut expect_output = false;

        for arg in args {
            if expect_output {
                output = Some(arg);
                expect_output = false;
                continue;
            }
            match arg.as_str() {
                "-h" | "--help" => return Err("help requested".to_string()),
                "-O0" => optimize = false,
                "-O1" => optimize = true,
                "--time" => show_timings = true,
                "-o" | "--output" => expect_output = true,
                flag if flag.starts_with('-') => return Err(format!("unknown option `{flag}`")),
                _ => positional.push(arg),
            }
        }

        let command_name = positional.first().ok_or("missing command")?.clone();

        // `explain` takes a code, not a file.
        if command_name == "explain" {
            let code = positional
                .get(1)
                .ok_or("`explain` needs a code, e.g. E0300")?
                .clone();
            return Ok(Cli {
                command: Command::Explain(code),
                path: String::new(),
                output,
                optimize,
                show_timings,
            });
        }

        let command = match command_name.as_str() {
            "run" => Command::Run,
            "check" => Command::Check,
            "fmt" => Command::Fmt,
            "build" => Command::Build,
            "exec" => Command::Exec,
            "dump" => {
                let what = positional.get(1).ok_or("`dump` needs a form to print")?;
                Command::Dump(parse_stage(what)?)
            }
            other => return Err(format!("unknown command `{other}`")),
        };

        let path_index = if matches!(command, Command::Dump(_)) {
            2
        } else {
            1
        };
        let path = positional
            .get(path_index)
            .ok_or("missing source file")?
            .clone();

        Ok(Cli {
            command,
            path,
            output,
            optimize,
            show_timings,
        })
    }
}

fn parse_stage(what: &str) -> Result<Stage, String> {
    Ok(match what {
        "tokens" => Stage::Tokens,
        "ast" => Stage::Ast,
        "hir" => Stage::Hir,
        "hir-opt" => Stage::OptimizedHir,
        "mir" => Stage::Mir,
        "cfg" => Stage::Cfg,
        "c" => Stage::C,
        "bytecode" => Stage::Bytecode,
        "verify" => Stage::Verify,
        other => {
            return Err(format!(
                "unknown dump form `{other}` (expected tokens|ast|hir|hir-opt|mir|cfg|c|bytecode|verify)"
            ));
        }
    })
}

const HELP: &str = "\
lumenc  the Lumen compiler

USAGE:
    lumenc run   <file.lm>          compile and execute
    lumenc check <file.lm>          type-check only
    lumenc fmt     <file.lm>        print canonically-formatted source
    lumenc build   <file.lm> -o <o> compile to a bytecode object file
    lumenc exec    <object>         run a built bytecode object file
    lumenc dump    <form> <file.lm> print an intermediate form
    lumenc explain <CODE>           explain a diagnostic code (e.g. E0300)

DUMP FORMS:
    tokens   ast   hir   hir-opt   mir   cfg   c   bytecode   verify

OPTIONS:
    -O0        disable optimization
    -O1        enable optimization (default)
    --time     print per-phase timings to stderr
    -h --help  show this help
";

fn run(cli: Cli) -> ExitCode {
    if let Command::Explain(code) = &cli.command {
        return match lumen::explain::explain(code) {
            Some(text) => {
                print!("{text}");
                ExitCode::SUCCESS
            }
            None => {
                eprintln!("error: unknown diagnostic code `{code}`");
                ExitCode::from(2)
            }
        };
    }

    if matches!(cli.command, Command::Exec) {
        return exec_object(&cli.path);
    }

    let src = match read_file(&cli.path) {
        Ok(src) => src,
        Err(code) => return code,
    };

    let stop_after = match &cli.command {
        Command::Run | Command::Check | Command::Build => Stage::Bytecode,
        Command::Fmt => Stage::Ast,
        Command::Dump(stage) => *stage,
        Command::Explain(_) | Command::Exec => unreachable!("handled earlier"),
    };
    let options = PipelineOptions {
        stop_after,
        optimize: OptOptions {
            enabled: cli.optimize,
            ..OptOptions::default()
        },
    };

    let mut session = Session::new(cli.path.clone(), src);
    let artifacts = session.compile(options);

    let rendered = session.render_diagnostics();
    if !rendered.is_empty() {
        eprint!("{rendered}");
    }
    if cli.show_timings {
        print_timings(&session);
    }
    if session.diagnostics().has_errors() {
        return ExitCode::from(1);
    }

    match cli.command {
        Command::Check => {
            eprintln!("ok: no errors");
            ExitCode::SUCCESS
        }
        Command::Dump(stage) => {
            print!("{}", render_dump(stage, &artifacts));
            ExitCode::SUCCESS
        }
        Command::Fmt => {
            if let Some(ast) = &artifacts.ast {
                print!("{}", lumen::format::format_source(ast));
            }
            ExitCode::SUCCESS
        }
        Command::Run => match &artifacts.program {
            Some(program) => run_program(program),
            None => no_program(),
        },
        Command::Build => build_object(&artifacts, cli.output.as_deref()),
        Command::Explain(_) | Command::Exec => unreachable!("handled earlier"),
    }
}

/// Reads a file, reporting failure and returning exit code 2.
fn read_file(path: &str) -> Result<String, ExitCode> {
    std::fs::read_to_string(path).map_err(|err| {
        eprintln!("error: cannot read `{path}`: {err}");
        ExitCode::from(2)
    })
}

fn no_program() -> ExitCode {
    eprintln!("internal error: no program was produced");
    ExitCode::from(1)
}

/// Writes the compiled program as an object file to `output`, or stdout.
fn build_object(artifacts: &lumen::Artifacts, output: Option<&str>) -> ExitCode {
    let Some(program) = &artifacts.program else {
        return no_program();
    };
    let text = lumen::backend::object::to_text(program);
    match output {
        Some(path) => match std::fs::write(path, text) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("error: cannot write `{path}`: {err}");
                ExitCode::from(2)
            }
        },
        None => {
            print!("{text}");
            ExitCode::SUCCESS
        }
    }
}

/// Loads, verifies, and runs an object file.
fn exec_object(path: &str) -> ExitCode {
    let text = match read_file(path) {
        Ok(text) => text,
        Err(code) => return code,
    };
    let program = match lumen::backend::object::from_text(&text) {
        Ok(p) => p,
        Err(err) => {
            eprintln!("error: invalid object file `{path}`: {err}");
            return ExitCode::from(2);
        }
    };
    // An object file is untrusted input.
    if let Err(err) = lumen::backend::verify(&program) {
        eprintln!("error: object file `{path}` failed verification: {err}");
        return ExitCode::from(2);
    }
    run_program(&program)
}

/// Runs `program`, printing its output, or on a runtime error only the error.
fn run_program(program: &Program) -> ExitCode {
    match execute(program) {
        Ok(execution) => {
            print!("{}", execution.stdout);
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("runtime error: {err}");
            ExitCode::from(1)
        }
    }
}

/// Renders the intermediate form `stage` names.
fn render_dump(stage: Stage, artifacts: &lumen::Artifacts) -> String {
    match stage {
        Stage::Tokens => artifacts
            .tokens
            .as_ref()
            .map(|toks| {
                toks.iter()
                    .map(|t| format!("{:>10?}  {:?}", t.span, t.kind))
                    .collect::<Vec<_>>()
                    .join("\n")
                    + "\n"
            })
            .unwrap_or_default(),
        Stage::Ast => artifacts.ast.as_ref().map(print_ast).unwrap_or_default(),
        Stage::Hir | Stage::OptimizedHir => {
            artifacts.hir.as_ref().map(print_hir).unwrap_or_default()
        }
        Stage::Mir => artifacts
            .hir
            .as_ref()
            .map(|hir| lumen::mir::print_mir(&optimized_mir(hir)))
            .unwrap_or_default(),
        Stage::Cfg => artifacts
            .hir
            .as_ref()
            .map(|hir| lumen::mir::to_dot(&optimized_mir(hir)))
            .unwrap_or_default(),
        Stage::C => artifacts
            .hir
            .as_ref()
            .map(|hir| match lumen::backend::emit_c(hir) {
                Ok(c) => c,
                Err(err) => format!("// C backend error: {err}\n"),
            })
            .unwrap_or_default(),
        Stage::Bytecode => artifacts
            .program
            .as_ref()
            .map(disassemble)
            .unwrap_or_default(),
        Stage::Verify => artifacts
            .program
            .as_ref()
            .map(|program| match lumen::backend::verify(program) {
                Ok(()) => "ok: bytecode verified\n".to_string(),
                Err(err) => format!("verification failed: {err}\n"),
            })
            .unwrap_or_default(),
    }
}

fn optimized_mir(hir: &lumen::hir::Hir) -> lumen::mir::Program {
    let mut mir = lumen::mir::build(hir);
    lumen::mir::optimize(&mut mir);
    mir
}

fn print_timings(session: &Session) {
    let timings = session.timings();
    eprintln!("phase timings:");
    for (name, dur) in timings.entries() {
        eprintln!("  {name:<10} {:>8} µs", dur.as_micros());
    }
    eprintln!("  {:<10} {:>8} µs", "total", timings.total().as_micros());
}
