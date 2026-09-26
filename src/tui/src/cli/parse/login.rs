//! Parsing for the `medulla login` command and its authentication options.

use medulla::auth::Provider;

use crate::cli::types::LoginArgs;

/// Parse `medulla login` flags out of the args following `login`. Returns the
/// offending flag name on an unknown `--provider` value.
pub fn parse_login_args(args: &[String]) -> Result<LoginArgs, String> {
    let mut out = LoginArgs::default();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--config" => {
                if let Some(v) = it.next() {
                    out.config = Some(v.clone());
                }
            }
            "--provider" => {
                if let Some(v) = it.next() {
                    out.provider =
                        Provider::parse(v).ok_or_else(|| format!("unknown provider '{v}'"))?;
                }
            }
            "--token" => {
                if let Some(v) = it.next() {
                    out.token = Some(v.clone());
                }
            }
            "--no-browser" => out.no_browser = true,
            "--code" => out.code = true,
            _ => {}
        }
    }
    Ok(out)
}
