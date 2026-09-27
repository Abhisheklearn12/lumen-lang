//! Diagnostics: the error and warning model shared by every phase, rendered
//! with [`miette`].
//!
//! Phases push [`Diagnostic`]s into a shared [`Diagnostics`] sink instead of
//! returning early, so one run reports every problem it can find. Rendering
//! uses a fixed, colourless theme so output is stable for tests.

use crate::errors::DiagCode;
use crate::source::SourceFile;
use crate::span::Span;

/// Whether a diagnostic stops compilation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

/// A message attached to a span. The primary label marks the problem;
/// secondary labels point at related code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Label {
    pub span: Span,
    pub message: String,
    pub primary: bool,
}

/// One error or warning. Build it with [`Diagnostic::error`] or
/// [`Diagnostic::warning`] and the `with_*` methods.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: DiagCode,
    pub message: String,
    pub labels: Vec<Label>,
    pub notes: Vec<String>,
    pub helps: Vec<String>,
}

impl Diagnostic {
    /// Starts an error with a headline `message`.
    pub fn error(code: DiagCode, message: impl Into<String>) -> Diagnostic {
        Diagnostic::new(Severity::Error, code, message.into())
    }

    /// Starts a warning with a headline `message`.
    pub fn warning(code: DiagCode, message: impl Into<String>) -> Diagnostic {
        Diagnostic::new(Severity::Warning, code, message.into())
    }

    fn new(severity: Severity, code: DiagCode, message: String) -> Diagnostic {
        Diagnostic {
            severity,
            code,
            message,
            labels: Vec::new(),
            notes: Vec::new(),
            helps: Vec::new(),
        }
    }

    /// Adds the primary label, drawn as the underline at the problem.
    pub fn with_primary(self, span: Span, message: impl Into<String>) -> Diagnostic {
        self.push_label(span, message.into(), true)
    }

    /// Adds a secondary label pointing at related code.
    pub fn with_label(self, span: Span, message: impl Into<String>) -> Diagnostic {
        self.push_label(span, message.into(), false)
    }

    fn push_label(mut self, span: Span, message: String, primary: bool) -> Diagnostic {
        self.labels.push(Label {
            span,
            message,
            primary,
        });
        self
    }

    /// Adds a note, rendered after the snippet as `note: …`.
    pub fn with_note(mut self, note: impl Into<String>) -> Diagnostic {
        self.notes.push(note.into());
        self
    }

    /// Adds a help line suggesting a fix.
    pub fn with_help(mut self, help: impl Into<String>) -> Diagnostic {
        self.helps.push(help.into());
        self
    }

    /// The primary label's span, else the first label's, else
    /// [`Span::DUMMY`]. Used to sort diagnostics into source order.
    pub fn primary_span(&self) -> Span {
        self.labels
            .iter()
            .find(|l| l.primary)
            .or_else(|| self.labels.first())
            .map(|l| l.span)
            .unwrap_or(Span::DUMMY)
    }

    /// Renders against `file` as plain, uncoloured text.
    pub fn render(&self, file: &SourceFile) -> String {
        let adapter = Adapter::new(self, file);
        let mut out = String::new();
        let handler = miette::GraphicalReportHandler::new()
            .with_theme(miette::GraphicalTheme::unicode_nocolor());
        // Writing into a `String` cannot fail.
        let _ = handler.render_report(&mut out, &adapter);
        out
    }
}

/// The sink every phase reports into. The driver checks
/// [`Diagnostics::has_errors`] between phases.
#[derive(Debug, Default)]
pub struct Diagnostics {
    items: Vec<Diagnostic>,
    errors: usize,
    warnings: usize,
}

impl Diagnostics {
    /// An empty sink.
    pub fn new() -> Diagnostics {
        Diagnostics::default()
    }

    /// Records a diagnostic.
    pub fn emit(&mut self, diag: Diagnostic) {
        match diag.severity {
            Severity::Error => self.errors += 1,
            Severity::Warning => self.warnings += 1,
        }
        self.items.push(diag);
    }

    /// Number of errors recorded.
    pub fn error_count(&self) -> usize {
        self.errors
    }

    /// Number of warnings recorded.
    pub fn warning_count(&self) -> usize {
        self.warnings
    }

    /// Whether any error was recorded.
    pub fn has_errors(&self) -> bool {
        self.errors > 0
    }

    /// Whether nothing was recorded.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Every diagnostic, in emission order.
    pub fn items(&self) -> &[Diagnostic] {
        &self.items
    }

    /// Renders every diagnostic in source order (by primary span; ties keep
    /// emission order), each followed by a blank line.
    pub fn render_all(&self, file: &SourceFile) -> String {
        let mut order: Vec<&Diagnostic> = self.items.iter().collect();
        order.sort_by_key(|d| d.primary_span().lo);
        let mut out = String::new();
        for diag in order {
            out.push_str(&diag.render(file));
            out.push('\n');
        }
        out
    }
}

/// Adapts a [`Diagnostic`] to [`miette::Diagnostic`]. Private, so nothing else
/// depends on miette.
#[derive(Debug)]
struct Adapter {
    severity: Severity,
    code: DiagCode,
    message: String,
    labels: Vec<miette::LabeledSpan>,
    help: Option<String>,
    source: miette::NamedSource<String>,
}

impl Adapter {
    fn new(diag: &Diagnostic, file: &SourceFile) -> Adapter {
        let labels = diag
            .labels
            .iter()
            .map(|l| {
                let span: miette::SourceSpan = (l.span.lo as usize, l.span.len() as usize).into();
                if l.primary {
                    miette::LabeledSpan::new_primary_with_span(Some(l.message.clone()), span)
                } else {
                    miette::LabeledSpan::new_with_span(Some(l.message.clone()), span)
                }
            })
            .collect();

        // miette has a single help block, so notes and helps share it; notes
        // get a `note:` prefix to tell them apart.
        let mut footer = Vec::new();
        footer.extend(diag.notes.iter().map(|n| format!("note: {n}")));
        footer.extend(diag.helps.iter().cloned());
        let help = if footer.is_empty() {
            None
        } else {
            Some(footer.join("\n"))
        };

        Adapter {
            severity: diag.severity,
            code: diag.code,
            message: diag.message.clone(),
            labels,
            help,
            source: miette::NamedSource::new(file.name(), file.text().to_owned()),
        }
    }
}

impl std::fmt::Display for Adapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Adapter {}

impl miette::Diagnostic for Adapter {
    fn code(&self) -> Option<Box<dyn std::fmt::Display + '_>> {
        Some(Box::new(self.code.as_str()))
    }

    fn severity(&self) -> Option<miette::Severity> {
        Some(match self.severity {
            Severity::Error => miette::Severity::Error,
            Severity::Warning => miette::Severity::Warning,
        })
    }

    fn help(&self) -> Option<Box<dyn std::fmt::Display + '_>> {
        self.help
            .as_ref()
            .map(|h| Box::new(h.clone()) as Box<dyn std::fmt::Display>)
    }

    fn source_code(&self) -> Option<&dyn miette::SourceCode> {
        Some(&self.source)
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = miette::LabeledSpan> + '_>> {
        Some(Box::new(self.labels.iter().cloned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sink_counts_and_flags() {
        let mut diags = Diagnostics::new();
        assert!(!diags.has_errors());
        diags.emit(Diagnostic::warning(DiagCode::MissingReturn, "w"));
        diags.emit(Diagnostic::error(DiagCode::TypeMismatch, "e"));
        assert_eq!(diags.warning_count(), 1);
        assert_eq!(diags.error_count(), 1);
        assert!(diags.has_errors());
    }

    #[test]
    fn renders_with_caret_and_code() {
        let file = SourceFile::new("main.lm", "let x: i32 = \"hello\";");
        let diag = Diagnostic::error(DiagCode::TypeMismatch, "mismatched types")
            .with_primary(Span::new(13, 20), "expected i32, found str")
            .with_help("change the literal to an integer");
        let out = diag.render(&file);
        assert!(out.contains("E0300"), "code missing:\n{out}");
        assert!(out.contains("mismatched types"), "message missing:\n{out}");
        assert!(
            out.contains("expected i32, found str"),
            "label missing:\n{out}"
        );
        assert!(out.contains("help:"), "help missing:\n{out}");
    }

    #[test]
    fn render_all_is_source_ordered() {
        let file = SourceFile::new("main.lm", "aaaa\nbbbb\n");
        let mut diags = Diagnostics::new();
        diags.emit(
            Diagnostic::error(DiagCode::UnresolvedName, "second")
                .with_primary(Span::new(5, 9), "here"),
        );
        diags.emit(
            Diagnostic::error(DiagCode::UnresolvedName, "first")
                .with_primary(Span::new(0, 4), "here"),
        );
        let out = diags.render_all(&file);
        let first = out.find("first").unwrap();
        let second = out.find("second").unwrap();
        assert!(first < second, "diagnostics not source-ordered:\n{out}");
    }
}
