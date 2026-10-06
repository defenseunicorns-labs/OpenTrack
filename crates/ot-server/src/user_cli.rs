//! `opentrack user`: manage accounts without the UI, for a headless node
//! (a vehicle, a remote site). Each change goes in the decision log.

use std::io::BufRead;

use clap::Subcommand;
use serde_json::json;

use crate::auth::{self, Role};
use crate::config::Common;

#[derive(Debug, Subcommand)]
pub enum UserCommand {
    /// Every account, its role and whether it is active.
    List,
    /// Add an account.
    Add {
        email: String,
        /// viewer, track_manager or admin.
        #[arg(long, value_parser = parse_role)]
        role: Role,
        #[arg(long, default_value = "")]
        name: String,
        /// Read its password from the first line of standard input (else
        /// it can only use single sign-on or an API token).
        #[arg(long)]
        password_stdin: bool,
        /// A temporary or emergency account: turned off 72 hours after it is
        /// made.
        #[arg(long)]
        temporary: bool,
    },
    /// Change an account's role.
    Role {
        email: String,
        #[arg(value_parser = parse_role)]
        role: Role,
    },
    /// Set an account's password from the first line of standard input;
    /// its sessions end. It is a temporary one, changed at the next sign-in.
    Passwd {
        email: String,
    },
    /// Unlock an account locked by failed sign-ins.
    Unlock {
        email: String,
    },
    /// Turn an account off (its sessions and tokens stop working) or on.
    Disable {
        email: String,
    },
    Enable {
        email: String,
    },
    /// Print a new API token acting as the account.
    Token {
        email: String,
        /// What it is for.
        #[arg(long)]
        name: String,
        #[arg(long, default_value_t = 365.0)]
        days: f64,
        /// The key the server signs tokens with, if not the one beside the
        /// database.
        #[arg(long, env = "OT_SESSION_SECRET", hide_env_values = true)]
        session_secret: Option<String>,
    },
}

fn parse_role(s: &str) -> Result<Role, String> {
    Role::parse(s).ok_or_else(|| format!("{s}: viewer, track_manager or admin"))
}

/// A password from standard input, checked against the password rules.
fn stdin_password() -> anyhow::Result<String> {
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    let p = line.trim_end_matches(['\r', '\n']).to_owned();
    auth::password::check(&p).map_err(anyhow::Error::msg)?;
    Ok(p)
}

const ACTOR: &str = "cli";

pub fn run(common: &Common, cmd: UserCommand) -> anyhow::Result<()> {
    let mut db = common.open_new_db()?;
    let find = |db: &ot_store::Db, email: &str| -> anyhow::Result<ot_store::User> {
        db.user_by_email(email)?
            .ok_or_else(|| anyhow::anyhow!("no account {email}"))
    };
    match cmd {
        UserCommand::List => {
            let now = ot_store::sqlite::now_ms();
            for u in db.users()? {
                println!(
                    "{:<32} {:<14} {:<6} {}{}{}{}",
                    u.email,
                    u.role,
                    u.origin,
                    if u.active { "active" } else { "off" },
                    match u.disabled_reason.as_deref() {
                        Some(r) if !u.active => format!(" ({r})"),
                        _ => String::new(),
                    },
                    if u.locked(now) { ", locked" } else { "" },
                    if u.has_password { "" } else { ", no password" },
                );
            }
        }
        UserCommand::Add {
            email,
            role,
            name,
            password_stdin,
            temporary,
        } => {
            let hash = if password_stdin {
                Some(auth::hash_password(&stdin_password()?)?)
            } else {
                None
            };
            let id = crate::fips::uuid_v4();
            let u = db.create_user(&ot_store::NewUser {
                id: &id,
                email: &email,
                name: &name,
                role: role.as_str(),
                password_hash: hash.as_deref(),
                origin: "local",
            })?;
            if hash.is_some() {
                db.set_must_change_password(&id, true)?;
            }
            let u = if temporary {
                let hours = auth::stig::TEMPORARY_ACCOUNT_HOURS;
                db.set_account_expiry(&id, Some(ot_store::sqlite::now_ms() + hours * 3_600_000))?
            } else {
                u
            };
            db.record(&ot_store::Decision {
                after: Some(json!(u)),
                ..ot_store::Decision::new(ACTOR, "create_user")
            })?;
            println!("added {} ({})", u.email, u.role);
            if temporary {
                println!(
                    "temporary: turned off {} hours from now",
                    auth::stig::TEMPORARY_ACCOUNT_HOURS
                );
            }
        }
        UserCommand::Role { email, role } => {
            let u = find(&db, &email)?;
            let after = db.update_user(&u.id, None, Some(role.as_str()), None)?;
            db.record(&ot_store::Decision {
                before: Some(json!(u)),
                after: Some(json!(after)),
                ..ot_store::Decision::new(ACTOR, "update_user")
            })?;
            println!("{email} is now {}", after.role);
            if after.origin == "saml" {
                println!(
                    "(a saml account: its next sign-on sets the identity provider's role again)"
                );
            }
        }
        UserCommand::Passwd { email } => {
            let u = find(&db, &email)?;
            let hash = auth::hash_password(&stdin_password()?)?;
            db.set_password(&u.id, Some(&hash), true, 24)?;
            db.revoke_user_tokens(&u.id)?;
            db.record(&ot_store::Decision {
                evidence: json!({ "user": u.id, "temporary": true }),
                ..ot_store::Decision::new(ACTOR, "reset_password")
            })?;
            println!(
                "password set for {email} (to be changed at its next sign-in); its sessions ended"
            );
        }
        UserCommand::Unlock { email } => {
            let u = find(&db, &email)?;
            let after = db.unlock_user(&u.id)?;
            db.record(&ot_store::Decision {
                evidence: json!({ "user": u.id, "email": u.email }),
                before: Some(json!(u)),
                after: Some(json!(after)),
                ..ot_store::Decision::new(ACTOR, "unlock_user")
            })?;
            println!("{email} is unlocked");
        }
        UserCommand::Disable { email } => {
            let u = find(&db, &email)?;
            set_active(&mut db, &u, false)?;
        }
        UserCommand::Enable { email } => {
            let u = find(&db, &email)?;
            set_active(&mut db, &u, true)?;
        }
        UserCommand::Token {
            email,
            name,
            days,
            session_secret,
        } => {
            let u = find(&db, &email)?;
            let secret = auth::load_secret(common, session_secret.as_deref())?;
            let a = auth::Auth::new(secret, false, false, None, Default::default());
            let (token, jti, exp) = a.issue(&u, "api", (days * 86_400.0) as i64)?;
            db.add_api_token(&jti, &name, &u.id, ACTOR, exp)?;
            db.record(&ot_store::Decision {
                evidence: json!({ "token": jti, "name": name, "user": u.id }),
                ..ot_store::Decision::new(ACTOR, "create_api_token")
            })?;
            println!("{token}");
        }
    }
    Ok(())
}

fn set_active(db: &mut ot_store::Db, u: &ot_store::User, active: bool) -> anyhow::Result<()> {
    let after = db.update_user(&u.id, None, None, Some(active))?;
    db.record(&ot_store::Decision {
        before: Some(json!(u)),
        after: Some(json!(after)),
        ..ot_store::Decision::new(ACTOR, "update_user")
    })?;
    println!("{} is {}", u.email, if active { "on" } else { "off" });
    Ok(())
}
