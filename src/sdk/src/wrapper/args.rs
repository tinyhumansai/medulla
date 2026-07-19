//! Command-line parsing for the wrapper entry point: split the wrapper's own
//! flags from the arguments passed through to the child CLI.

/// Parse `medulla <provider> [args…]`: strip the wrapper's own `--no-bridge`, pass
/// everything else through to the child verbatim. `--` forces the rest through.
pub fn parse_wrapper_args(args: &[String]) -> (bool, Vec<String>) {
    let mut no_bridge = false;
    let mut child: Vec<String> = Vec::new();
    let mut passthrough = false;
    for arg in args {
        if passthrough {
            child.push(arg.clone());
            continue;
        }
        match arg.as_str() {
            "--" => passthrough = true,
            "--no-bridge" => no_bridge = true,
            _ => child.push(arg.clone()),
        }
    }
    (no_bridge, child)
}
