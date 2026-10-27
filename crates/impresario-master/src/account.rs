//! `impresario account`: the machine-global Apple ID login wizard.
//!
//! This command deliberately runs before workspace config resolution. The
//! Apple session is stored once under `~/.impresario/apple/`, encrypted with a
//! key held by the OS credential store, and reused by every workspace.

use std::io::{self, Write};

use clap::{Args, Subcommand, ValueEnum};
use impresario_apple_auth::account::{
    authenticate, choose_team, masked_number, verify, AnisetteConfig, AppleCredentials, AppleStore,
    AuthError, CredentialStore, LoginSession, TwoFactorPrompt, DEFAULT_ANISETTE_URL,
};
use impresario_apple_auth::auth::apple_account::{
    TrustedNumber, TwoFactorCallbackParams, TwoFactorCallbackResponse,
};
use impresario_apple_auth::dev::teams::DeveloperTeam;
use impresario_core::home::Home;
use zeroize::Zeroize;

use crate::cli::{EXIT_FAILURE, EXIT_INFRA, EXIT_OK, EXIT_USAGE};

/// `impresario account ...`
#[derive(Debug, Args)]
pub struct AccountArgs {
    #[command(subcommand)]
    pub command: AccountCommand,
}

#[derive(Debug, Subcommand)]
pub enum AccountCommand {
    /// Sign in once and store the Apple session globally (alias: wizard).
    #[command(visible_alias = "wizard")]
    Login(LoginArgs),
    /// Show the globally stored Apple account.
    Status(StatusArgs),
    /// Remove the globally stored Apple account.
    Logout(LogoutArgs),
}

#[derive(Debug, Args)]
pub struct LoginArgs {
    /// Apple ID email. Prompted when omitted and no account is stored.
    #[arg(long)]
    pub email: Option<String>,
    /// Sign in again even when a stored session already exists.
    #[arg(long)]
    pub force: bool,
    /// Remote anisette v3 service (defaults to the stored value, then the
    /// built-in endpoint). Override for a self-hosted anisette server.
    #[arg(long, env = "IMPRESARIO_ANISETTE_URL")]
    pub anisette_url: Option<String>,
    /// Stable anisette machine serial (defaults to the stored value, then 0).
    #[arg(long)]
    pub serial: Option<String>,
    /// Developer team id when the Apple ID belongs to several teams.
    #[arg(long)]
    pub team: Option<String>,
    /// Preferred two-factor method. `ask` shows the trusted-device/SMS menu.
    #[arg(long, value_enum, default_value = "ask")]
    pub two_factor: TwoFactorChoice,
    /// Trusted SMS number id to use with `--two-factor sms` (see the printed
    /// number list; defaults to the first trusted number).
    #[arg(long)]
    pub two_factor_number: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum TwoFactorChoice {
    /// Show the trusted-device/SMS menu.
    Ask,
    /// Prefer a trusted device; fall back to SMS if sending fails.
    Device,
    /// Prefer one trusted SMS number; fall back to a trusted device if sending
    /// fails.
    Sms,
}

#[derive(Debug, Args)]
pub struct StatusArgs {
    /// Also validate the session against Apple (refreshes the developer token
    /// from the stored `spd` when needed).
    #[arg(long)]
    pub online: bool,
    /// Machine-readable JSON with no token or session material.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct LogoutArgs {
    /// Do not ask for confirmation.
    #[arg(long)]
    pub yes: bool,
}

/// Run the account command. `home` is the machine-global Impresario home;
/// workspace selection is intentionally ignored.
pub fn run(args: &AccountArgs, home: &Home) -> i32 {
    let store = AppleStore::new(home.root().join("apple"));
    match &args.command {
        AccountCommand::Login(login) => run_login(login, &store),
        AccountCommand::Status(status) => run_status(status, &store),
        AccountCommand::Logout(logout) => run_logout(logout, &store),
    }
}

fn run_login(args: &LoginArgs, store: &AppleStore) -> i32 {
    let existing = match store.load() {
        Ok(existing) => existing,
        Err(error) if args.force => {
            eprintln!("warning: replacing the stored Apple session: {error}");
            None
        }
        Err(error) => {
            eprintln!("error: {error}");
            eprintln!("Run `impresario account login --force` to replace the broken session.");
            return EXIT_FAILURE;
        }
    };

    if let Some(existing) = &existing {
        if !args.force {
            print_summary(existing, store);
            match confirm("Sign in again? [y/N]: ", false) {
                Ok(true) => {}
                Ok(false) => {
                    println!("Keeping the existing Apple session.");
                    return EXIT_OK;
                }
                Err(error) => {
                    eprintln!("error: could not read confirmation: {error}");
                    return EXIT_INFRA;
                }
            }
        }
    }

    let email = match args.email.clone().or_else(|| {
        existing
            .as_ref()
            .map(|credentials| credentials.email.clone())
    }) {
        Some(email) if email.contains('@') => email,
        Some(_) => {
            eprintln!("error: the Apple ID email does not look valid");
            return EXIT_USAGE;
        }
        None => match prompt_value("Apple ID email: ") {
            Ok(Some(email)) if email.contains('@') => email,
            Ok(Some(_)) => {
                eprintln!("error: the Apple ID email does not look valid");
                return EXIT_USAGE;
            }
            Ok(None) => {
                eprintln!("error: no email entered");
                return EXIT_USAGE;
            }
            Err(error) => {
                eprintln!("error: could not read the Apple ID email: {error}");
                return EXIT_INFRA;
            }
        },
    };

    let mut password = match rpassword::prompt_password("Apple ID password (not stored): ") {
        Ok(password) => password,
        Err(error) => {
            eprintln!("error: could not read the Apple ID password: {error}");
            return EXIT_INFRA;
        }
    };
    if password.is_empty() {
        eprintln!("error: the Apple ID password was empty");
        password.zeroize();
        return EXIT_USAGE;
    }

    let anisette = AnisetteConfig {
        url: args
            .anisette_url
            .clone()
            .or_else(|| {
                existing
                    .as_ref()
                    .map(|credentials| credentials.anisette_url.clone())
            })
            .unwrap_or_else(|| DEFAULT_ANISETTE_URL.to_string()),
        serial_number: args
            .serial
            .clone()
            .or_else(|| {
                existing
                    .as_ref()
                    .map(|credentials| credentials.machine_serial.clone())
            })
            .unwrap_or_else(|| "0".to_string()),
    };

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("error: could not start the async runtime: {error}");
            password.zeroize();
            return EXIT_INFRA;
        }
    };

    println!("Signing in to Apple ID {email}...");
    println!("Apple may send a 6-digit code to a trusted device or a trusted SMS number.");
    if args.two_factor == TwoFactorChoice::Sms {
        println!("Preferred 2FA method: SMS.");
    } else if args.two_factor == TwoFactorChoice::Device {
        println!("Preferred 2FA method: trusted device.");
    }
    let login = runtime.block_on(authenticate(
        &email,
        &password,
        anisette,
        store.anisette_storage(),
        ConsoleTwoFactorPrompt {
            choice: args.two_factor,
            number: args.two_factor_number,
        },
    ));
    password.zeroize();

    let login = match login {
        Ok(login) => login,
        Err(error) => {
            print_login_error(&error);
            return EXIT_FAILURE;
        }
    };

    let team = match select_team(&login, args.team.as_deref()) {
        Ok(team) => team,
        Err(error) => {
            eprintln!("error: {error}");
            return EXIT_FAILURE;
        }
    };

    match login.save(&team, store) {
        Ok(credentials) => {
            print_success(&credentials, store);
            EXIT_OK
        }
        Err(error) => {
            eprintln!("error: could not save the Apple session: {error}");
            EXIT_INFRA
        }
    }
}

fn run_status(args: &StatusArgs, store: &AppleStore) -> i32 {
    let mut credentials = match store.load() {
        Ok(Some(credentials)) => credentials,
        Ok(None) => {
            if args.json {
                println!("{{\"logged_in\":false}}");
            } else {
                println!("No Apple account is stored globally.");
                println!("Run `impresario account login` to sign in once.");
            }
            return EXIT_OK;
        }
        Err(error) => {
            eprintln!("error: {error}");
            return EXIT_INFRA;
        }
    };

    if args.online {
        let runtime = match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => {
                eprintln!("error: could not start the async runtime: {error}");
                return EXIT_INFRA;
            }
        };
        if let Err(error) =
            runtime.block_on(verify(&mut credentials, store, store.anisette_storage()))
        {
            print_login_error(&error);
            return EXIT_FAILURE;
        }
    }

    if args.json {
        let value = serde_json::json!({
            "logged_in": true,
            "email": &credentials.email,
            "display_name": &credentials.display_name,
            "team_id": &credentials.team_id,
            "team_name": &credentials.team_name,
            "expires_at_unix": credentials.expires_at_unix(),
            "session_file": store.credentials_path().display().to_string(),
            "anisette_url": &credentials.anisette_url,
            "password_stored": false,
        });
        match serde_json::to_string_pretty(&value) {
            Ok(json) => println!("{json}"),
            Err(error) => {
                eprintln!("error: could not encode status: {error}");
                return EXIT_INFRA;
            }
        }
    } else {
        print_summary(&credentials, store);
        if args.online {
            println!("Session verified with Apple.");
        }
    }
    EXIT_OK
}

fn run_logout(args: &LogoutArgs, store: &AppleStore) -> i32 {
    match store.load() {
        Ok(Some(credentials)) => {
            if !args.yes {
                println!(
                    "Sign out {} from this machine's global Apple session?",
                    credentials.email
                );
                match confirm("Remove it? [y/N]: ", false) {
                    Ok(true) => {}
                    Ok(false) => {
                        println!("Keeping the Apple session.");
                        return EXIT_OK;
                    }
                    Err(error) => {
                        eprintln!("error: could not read confirmation: {error}");
                        return EXIT_INFRA;
                    }
                }
            }
        }
        Ok(None) => {
            println!("No Apple account is stored globally; nothing to remove.");
            return EXIT_OK;
        }
        Err(error) => {
            eprintln!("error: {error}");
            return EXIT_INFRA;
        }
    }

    match store.logout() {
        Ok(()) => {
            println!("Removed the global Apple session and anisette state.");
            println!("No workspace configuration was changed.");
            EXIT_OK
        }
        Err(error) => {
            eprintln!("error: could not remove the Apple session: {error}");
            EXIT_INFRA
        }
    }
}

fn select_team(login: &LoginSession, requested: Option<&str>) -> Result<DeveloperTeam, String> {
    let teams = login.teams();
    if let Some(team_id) = requested {
        return choose_team(teams, Some(team_id)).map_err(|error| error.to_string());
    }
    match choose_team(teams, None) {
        Ok(team) => return Ok(team),
        Err(AuthError::TeamChoiceRequired(_)) => {}
        Err(error) => return Err(error.to_string()),
    }

    println!("Apple developer teams:");
    for (index, team) in teams.iter().enumerate() {
        println!(
            "  {}) {} ({})",
            index.saturating_add(1),
            team.name.as_deref().unwrap_or("<unnamed>"),
            team.team_id
        );
    }
    loop {
        match prompt_value("Select team number: ") {
            Ok(Some(value)) => match value.parse::<usize>() {
                Ok(choice) if choice >= 1 && choice <= teams.len() => {
                    if let Some(team) = teams.get(choice.saturating_sub(1)) {
                        return Ok(team.clone());
                    }
                }
                _ => eprintln!("Enter a number between 1 and {}.", teams.len()),
            },
            Ok(None) => return Err("team selection was cancelled".to_string()),
            Err(error) => return Err(format!("could not read the team selection: {error}")),
        }
    }
}

fn print_summary(credentials: &AppleCredentials, store: &AppleStore) {
    let name = credentials
        .display_name
        .as_deref()
        .map(|name| format!("{name} <{}>", credentials.email))
        .unwrap_or_else(|| credentials.email.clone());
    println!("Apple account (global): {name}");
    println!(
        "Developer team: {} ({})",
        credentials.team_name.as_deref().unwrap_or("<unnamed>"),
        credentials.team_id
    );
    println!(
        "Developer token expires: {}",
        format_expiry(credentials.expires_at_unix())
    );
    println!("Session file: {}", store.credentials_path().display());
    println!("Password stored: no");
}

fn print_success(credentials: &AppleCredentials, store: &AppleStore) {
    println!();
    println!(
        "Signed in as {}.",
        credentials
            .display_name
            .as_deref()
            .map(|name| format!("{name} <{}>", credentials.email))
            .unwrap_or_else(|| credentials.email.clone())
    );
    println!(
        "Developer team: {} ({})",
        credentials.team_name.as_deref().unwrap_or("<unnamed>"),
        credentials.team_id
    );
    println!(
        "Developer token expires: {}",
        format_expiry(credentials.expires_at_unix())
    );
    println!(
        "Global session: {} (encrypted; the key is in the OS credential store)",
        store.credentials_path().display()
    );
    println!("No Apple ID password was stored.");
    println!(
        "This account is available to every workspace; log in again only if Apple invalidates the session."
    );
}

fn print_login_error(error: &AuthError) {
    eprintln!("error: {error}");
    match error {
        AuthError::TwoFactorCancelled => {
            eprintln!("Apple sign-in was cancelled; no new session was saved.");
        }
        AuthError::NoTeams => {
            eprintln!(
                "Open https://developer.apple.com/account, accept any agreement, then retry."
            );
        }
        AuthError::MissingKey | AuthError::CorruptStore => {
            eprintln!("Run `impresario account login --force` to replace the stored session.");
        }
        AuthError::Apple(_) | AuthError::Expired => {
            eprintln!(
                "If Apple invalidated the stored session, run `impresario account login --force`."
            );
        }
        AuthError::Keyring(_) | AuthError::Storage(_) | AuthError::Prompt(_) => {
            eprintln!(
                "The OS credential store must be available; check the session file permissions."
            );
        }
        AuthError::UnknownTeam(_) | AuthError::TeamChoiceRequired(_) | AuthError::NotLoggedIn => {}
    }
}

fn format_expiry(seconds: u64) -> String {
    if seconds == 0 {
        "unknown".to_string()
    } else {
        crate::time_fmt::rfc3339_utc(seconds.saturating_mul(1000))
    }
}

fn prompt_value(prompt: &str) -> io::Result<Option<String>> {
    print!("{prompt}");
    io::stdout().flush()?;
    let mut line = String::new();
    if io::stdin().read_line(&mut line)? == 0 {
        return Ok(None);
    }
    Ok(Some(line.trim().to_string()))
}

fn confirm(prompt: &str, default: bool) -> io::Result<bool> {
    loop {
        let Some(value) = prompt_value(prompt)? else {
            return Ok(default);
        };
        match value.to_ascii_lowercase().as_str() {
            "" => return Ok(default),
            "y" | "yes" => return Ok(true),
            "n" | "no" => return Ok(false),
            _ => eprintln!("Please answer y or n."),
        }
    }
}

/// Console 2FA. Apple can require a code from a trusted device, or let the
/// user choose one of the trusted SMS numbers. `--two-factor` can make the
/// choice up front; interactive fallbacks remain available.
struct ConsoleTwoFactorPrompt {
    choice: TwoFactorChoice,
    number: Option<u32>,
}

impl TwoFactorPrompt for ConsoleTwoFactorPrompt {
    fn ask(
        &mut self,
        params: &TwoFactorCallbackParams,
    ) -> Result<TwoFactorCallbackResponse, AuthError> {
        if let Some(error) = &params.last_error {
            eprintln!("Apple: {error}");
        }

        if params.unknown {
            // If a forced method just failed, try the other method once before
            // asking again. This keeps `--two-factor device|sms` usable even
            // when Apple refuses the preferred method.
            if self.choice == TwoFactorChoice::Device
                && params
                    .last_error
                    .as_deref()
                    .is_some_and(|error| error.starts_with("Could not send a trusted-device"))
            {
                if let Ok(id) = choose_number_auto(&params.numbers, self.number) {
                    return Ok(TwoFactorCallbackResponse::SendSms(id));
                }
            }
            if self.choice == TwoFactorChoice::Sms
                && params
                    .last_error
                    .as_deref()
                    .is_some_and(|error| error.starts_with("Could not send an SMS"))
            {
                return Ok(TwoFactorCallbackResponse::SendToDevices);
            }

            match self.choice {
                TwoFactorChoice::Device => {
                    return Ok(TwoFactorCallbackResponse::SendToDevices);
                }
                TwoFactorChoice::Sms => {
                    return Ok(TwoFactorCallbackResponse::SendSms(choose_number_auto(
                        &params.numbers,
                        self.number,
                    )?));
                }
                TwoFactorChoice::Ask => {
                    println!("Apple needs a verification method.");
                    println!("  1) Send a code to my trusted devices");
                    println!("  2) Send a code by SMS to a trusted number");
                    println!("  q) Cancel");
                    loop {
                        let input = read_line("Choose [1/2/q]: ")?;
                        match input.as_str() {
                            "1" | "d" => return Ok(TwoFactorCallbackResponse::SendToDevices),
                            "2" | "s" => match choose_number(&params.numbers)? {
                                Some(id) => return Ok(TwoFactorCallbackResponse::SendSms(id)),
                                None => println!("Choose a trusted device instead."),
                            },
                            "q" | "quit" => return Err(AuthError::TwoFactorCancelled),
                            _ => println!("Enter 1, 2 or q."),
                        }
                    }
                }
            }
        }

        if params.sms {
            if self.choice == TwoFactorChoice::Device {
                return Ok(TwoFactorCallbackResponse::SendToDevices);
            }
            if let Some(number) = params
                .selected_number_id
                .and_then(|id| params.numbers.iter().find(|number| number.id == id))
            {
                println!("Apple sent a code by SMS to {}.", masked_number(number));
            } else {
                println!("Apple sent a code by SMS to a trusted number.");
            }
            loop {
                let input = read_line(
                    "Enter the SMS code (s = choose number, r = resend, d = trusted device, q = cancel): ",
                )?;
                if is_code(&input) {
                    return Ok(TwoFactorCallbackResponse::SubmitCode(input));
                }
                match input.as_str() {
                    "s" => match choose_number(&params.numbers)? {
                        Some(id) => return Ok(TwoFactorCallbackResponse::SendSms(id)),
                        None => println!("Keeping the current SMS number."),
                    },
                    "r" => return Ok(TwoFactorCallbackResponse::ResendCode),
                    "d" => return Ok(TwoFactorCallbackResponse::SendToDevices),
                    "q" | "quit" => return Err(AuthError::TwoFactorCancelled),
                    _ => println!("Enter the numeric code, or one of s/r/d/q."),
                }
            }
        }

        if self.choice == TwoFactorChoice::Sms {
            return Ok(TwoFactorCallbackResponse::SendSms(choose_number_auto(
                &params.numbers,
                self.number,
            )?));
        }

        println!("Apple sent a code to your trusted devices.");
        if !params.numbers.is_empty() {
            println!("You can switch to SMS with `s`.");
        }
        loop {
            let input =
                read_line("Enter the verification code (s = SMS, r = resend, q = cancel): ")?;
            if is_code(&input) {
                return Ok(TwoFactorCallbackResponse::SubmitCode(input));
            }
            match input.as_str() {
                "s" => match choose_number(&params.numbers)? {
                    Some(id) => return Ok(TwoFactorCallbackResponse::SendSms(id)),
                    None => println!("No SMS number was selected."),
                },
                "r" => return Ok(TwoFactorCallbackResponse::ResendCode),
                "q" | "quit" => return Err(AuthError::TwoFactorCancelled),
                _ => println!("Enter the numeric code, or one of s/r/q."),
            }
        }
    }
}

/// Select a trusted SMS number without prompting. `requested` is an Apple
/// phone-number id; when omitted, the first advertised number is used.
fn choose_number_auto(numbers: &[TrustedNumber], requested: Option<u32>) -> Result<u32, AuthError> {
    if numbers.is_empty() {
        return Err(AuthError::Prompt(
            "Apple did not advertise a trusted SMS number; use a trusted device or add an SMS number to the Apple ID"
                .to_string(),
        ));
    }
    let number = match requested {
        Some(id) => numbers
            .iter()
            .find(|number| number.id == id)
            .ok_or_else(|| {
                let available = numbers
                    .iter()
                    .map(|number| format!("{} (id {})", masked_number(number), number.id))
                    .collect::<Vec<_>>()
                    .join(", ");
                AuthError::Prompt(format!(
                    "trusted SMS number id {id} is not on this Apple ID; available: {available}"
                ))
            })?,
        None => numbers
            .first()
            .ok_or_else(|| AuthError::Prompt("no trusted SMS number is available".to_string()))?,
    };
    println!("Using SMS number {}.", masked_number(number));
    Ok(number.id)
}

fn choose_number(numbers: &[TrustedNumber]) -> Result<Option<u32>, AuthError> {
    if numbers.is_empty() {
        println!("No trusted SMS numbers are available on this Apple ID.");
        return Ok(None);
    }
    println!("Trusted SMS numbers:");
    for (index, number) in numbers.iter().enumerate() {
        println!("  {}) {}", index.saturating_add(1), masked_number(number));
    }
    println!("  b) Back");
    println!("  q) Cancel");
    loop {
        let input = read_line("Select a number [1-N]: ")?;
        if input == "b" {
            return Ok(None);
        }
        if input == "q" || input == "quit" {
            return Err(AuthError::TwoFactorCancelled);
        }
        if let Ok(choice) = input.parse::<usize>() {
            if choice >= 1 && choice <= numbers.len() {
                if let Some(number) = numbers.get(choice.saturating_sub(1)) {
                    println!("Using {}.", masked_number(number));
                    return Ok(Some(number.id));
                }
            }
        }
        eprintln!("Enter a number between 1 and {}, b, or q.", numbers.len());
    }
}

fn read_line(prompt: &str) -> Result<String, AuthError> {
    print!("{prompt}");
    io::stdout()
        .flush()
        .map_err(|error| AuthError::Prompt(error.to_string()))?;
    let mut line = String::new();
    let read = io::stdin()
        .read_line(&mut line)
        .map_err(|error| AuthError::Prompt(error.to_string()))?;
    if read == 0 {
        return Err(AuthError::TwoFactorCancelled);
    }
    Ok(line.trim().to_ascii_lowercase())
}

fn is_code(value: &str) -> bool {
    let length = value.len();
    (4..=8).contains(&length) && value.chars().all(|character| character.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn number(id: u32, last_two_digits: &str) -> TrustedNumber {
        TrustedNumber {
            number_with_dial_code: "+1 555 0100".to_string(),
            last_two_digits: last_two_digits.to_string(),
            push_mode: "sms".to_string(),
            id,
        }
    }

    #[test]
    fn auto_sms_prefers_the_requested_apple_number_id() {
        let numbers = vec![number(3, "03"), number(7, "07")];
        assert_eq!(choose_number_auto(&numbers, Some(7)).unwrap(), 7);
        assert_eq!(choose_number_auto(&numbers, None).unwrap(), 3);
    }

    #[test]
    fn auto_sms_rejects_an_unknown_or_missing_number() {
        let numbers = vec![number(3, "03")];
        assert!(choose_number_auto(&numbers, Some(9)).is_err());
        assert!(choose_number_auto(&[], None).is_err());
    }
}
