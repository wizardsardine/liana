//! Command-line argument parsing shared between liana-gui and liana-business.

use std::{error::Error, fmt::Display, path::PathBuf, process, str::FromStr};

use liana::miniscript::bitcoin::Network;

use crate::{dir::LianaDirectory, gui::Config};

/// Parsed command-line argument.
#[derive(Debug, Clone, PartialEq)]
pub enum Arg {
    DatadirPath(LianaDirectory),
    Network(Network),
}

/// Parse command-line arguments.
///
/// # Arguments
/// - `args`: Command-line arguments (including program name at args[0])
/// - `version`: Version to display for --version flag
/// - `available_networks`: Networks shown in help and accepted as arguments
/// - `default_network`: Network to mark as "(default)" in help text
pub fn parse_args(
    args: Vec<String>,
    version: impl Display,
    available_networks: &[Network],
    default_network: Option<Network>,
) -> Result<Vec<Arg>, Box<dyn Error>> {
    let mut res = Vec::new();

    let app_name = std::path::Path::new(&args[0])
        .file_name()
        .and_then(|s| s.to_str())
        // This should never happen
        .unwrap_or("liana");

    if args.len() > 1 && (args[1] == "--version" || args[1] == "-v") {
        eprintln!("{}", version);
        process::exit(0);
    }

    if args.len() > 1 && (args[1] == "--help" || args[1] == "-h") {
        let network_options: String = available_networks
            .iter()
            .map(|n| {
                let name = n.to_string().to_lowercase();
                let default_marker = if Some(*n) == default_network {
                    " (default)"
                } else {
                    ""
                };
                format!("    --{:<15} Use {} network{}", name, name, default_marker)
            })
            .collect::<Vec<_>>()
            .join("\n");

        eprintln!(
            r#"
Usage: {app_name} [OPTIONS]

Options:
    --datadir <PATH>    Path of liana datadir
    -v, --version       Display {app_name} version
    -h, --help          Print help
{network_options}
        "#
        );
        process::exit(0);
    }

    for (i, arg) in args.iter().enumerate() {
        if arg == "--datadir" {
            if let Some(a) = args.get(i + 1) {
                res.push(Arg::DatadirPath(LianaDirectory::new(PathBuf::from(a))));
            } else {
                return Err("missing arg to --datadir".into());
            }
        } else if arg.starts_with("--") && arg != "--datadir" {
            let network = Network::from_str(arg.trim_start_matches("--"))?;
            if !available_networks.contains(&network) {
                return Err(format!("network {} is not available", network).into());
            }
            res.push(Arg::Network(network));
        }
    }

    Ok(res)
}

/// Convert parsed command-line arguments to a Config.
///
/// # Arguments
/// - `args`: Parsed command-line arguments
/// - `default_network`: Network to use when none is specified in args
pub fn args_to_config(
    args: &[Arg],
    default_network: Option<Network>,
    app_name: String,
) -> Result<Config, Box<dyn Error>> {
    let app_name = app_name.to_string();
    match args {
        [] => {
            let datadir_path = LianaDirectory::new_default().unwrap();
            Ok(Config::new(datadir_path, default_network, app_name))
        }
        [Arg::Network(network)] => {
            let datadir_path = LianaDirectory::new_default().unwrap();
            Ok(Config::new(datadir_path, Some(*network), app_name))
        }
        [Arg::DatadirPath(datadir_path)] => {
            Ok(Config::new(datadir_path.clone(), default_network, app_name))
        }
        [Arg::DatadirPath(datadir_path), Arg::Network(network)]
        | [Arg::Network(network), Arg::DatadirPath(datadir_path)] => {
            Ok(Config::new(datadir_path.clone(), Some(*network), app_name))
        }
        _ => Err("Unknown args combination".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Network::{Bitcoin, Regtest, Signet, Testnet};

    const ALL_NETWORKS: &[Network] = &[Bitcoin, Testnet, Signet, Regtest];
    const VERSION: &str = "1.0.0";

    #[test]
    fn test_parse_args() {
        assert!(parse_args(
            vec!["app".into(), "--meth".into()],
            VERSION,
            ALL_NETWORKS,
            None
        )
        .is_err());
        assert!(parse_args(
            vec!["app".into(), "--datadir".into()],
            VERSION,
            ALL_NETWORKS,
            None
        )
        .is_err());
        assert_eq!(
            Some(vec![Arg::Network(Regtest)]),
            parse_args(
                vec!["app".into(), "--regtest".into()],
                VERSION,
                ALL_NETWORKS,
                None
            )
            .ok()
        );
        assert_eq!(
            Some(vec![
                Arg::DatadirPath(LianaDirectory::new(PathBuf::from("hello"))),
                Arg::Network(Testnet)
            ]),
            parse_args(
                "app --datadir hello --testnet"
                    .split(' ')
                    .map(|a| a.to_string())
                    .collect(),
                VERSION,
                ALL_NETWORKS,
                None
            )
            .ok()
        );
        assert_eq!(
            Some(vec![
                Arg::Network(Testnet),
                Arg::DatadirPath(LianaDirectory::new(PathBuf::from("hello"))),
            ]),
            parse_args(
                "app --testnet --datadir hello"
                    .split(' ')
                    .map(|a| a.to_string())
                    .collect(),
                VERSION,
                ALL_NETWORKS,
                None
            )
            .ok()
        );
    }

    #[test]
    fn test_parse_args_limited_networks() {
        // Test with limited networks (like liana-business)
        let limited_networks = &[Bitcoin, Signet];

        // Should accept Bitcoin
        assert_eq!(
            Some(vec![Arg::Network(Bitcoin)]),
            parse_args(
                vec!["app".into(), "--bitcoin".into()],
                VERSION,
                limited_networks,
                Some(Signet)
            )
            .ok()
        );

        // Should reject Testnet (not in available_networks)
        assert!(parse_args(
            vec!["app".into(), "--testnet".into()],
            VERSION,
            limited_networks,
            Some(Signet)
        )
        .is_err());
    }
}
