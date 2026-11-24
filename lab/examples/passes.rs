//! Write the per-pass TSV to stdout and the baseline/frozen evaluation to stderr.
fn main() -> Result<(), String> {
    let passes = std::env::args()
        .nth(1)
        .map(|arg| arg.parse::<usize>())
        .transpose()
        .map_err(|err| err.to_string())?
        .unwrap_or(150);
    let report = tutor_lab::campaign::run(passes)?;
    print!("{}", report.ledger);
    eprint!("{}", report.summary);
    if std::env::args().nth(2).as_deref() == Some("--diagnose") {
        eprint!("{}", report.diagnostics()?);
    }
    Ok(())
}
