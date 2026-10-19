//! Dealing with reports, from a terminal in the API's container (Coolify →
//! the app → Terminal):
//!
//! ```text
//! hasteclip-api reports           the reports nobody has dealt with
//! hasteclip-api remove <clip>     take the clip down (and close its reports)
//! hasteclip-api ban <clip>        take down everything its uploader shared,
//!                                 and stop them sharing more
//! hasteclip-api dismiss <clip>    close the clip's reports, keeping it up
//! ```

use time::macros::format_description;

use crate::{App, cleanup, db};

pub async fn run(app: &App, args: &[String]) -> Result<(), String> {
    let clip_arg = || args.get(1).map(String::as_str).ok_or("which clip? Give its id, like aB3xY9k2Qp");
    match args.first().map(String::as_str) {
        Some("reports") => reports(app).await,
        Some("remove") => {
            let clip = find(app, clip_arg()?).await?;
            cleanup::remove(app, &clip).await?;
            let closed = db::resolve_reports(&app.db, &clip.id).await.map_err(|e| e.to_string())?;
            println!("Removed {} (\"{}\"), closed {closed} report(s).", clip.id, clip.title);
            Ok(())
        }
        Some("ban") => {
            let clip = find(app, clip_arg()?).await?;
            db::ban(&app.db, clip.uploader_id).await.map_err(|e| e.to_string())?;
            let all = db::clips_of(&app.db, clip.uploader_id).await.map_err(|e| e.to_string())?;
            for c in &all {
                cleanup::remove(app, c).await?;
                db::resolve_reports(&app.db, &c.id).await.map_err(|e| e.to_string())?;
            }
            println!("Banned uploader {} and removed all {} of their clips.", clip.uploader_id, all.len());
            Ok(())
        }
        Some("dismiss") => {
            let id = clip_arg()?;
            let closed = db::resolve_reports(&app.db, id).await.map_err(|e| e.to_string())?;
            println!("Closed {closed} report(s) on {id}.");
            Ok(())
        }
        _ => Err("commands: reports, remove <clip>, ban <clip>, dismiss <clip>".into()),
    }
}

async fn find(app: &App, id: &str) -> Result<db::Clip, String> {
    db::clip(&app.db, id).await.map_err(|e| e.to_string())?.ok_or_else(|| format!("no clip {id}"))
}

async fn reports(app: &App) -> Result<(), String> {
    let reports = db::open_reports(&app.db).await.map_err(|e| e.to_string())?;
    if reports.is_empty() {
        println!("No open reports.");
    }
    let when = format_description!("[year]-[month]-[day] [hour]:[minute]");
    for r in reports {
        let shared = db::clip(&app.db, &r.clip_id).await.map_err(|e| e.to_string())?.is_some();
        println!(
            "#{} {} {} {}{}",
            r.id,
            r.created_at.format(when).unwrap_or_default(),
            app.link(&r.clip_id),
            r.reason,
            if shared { "" } else { " (already gone)" }
        );
        if !r.details.is_empty() {
            println!("    {}", r.details.replace('\n', "\n    "));
        }
        if let Some(email) = r.email {
            println!("    reply to: {email}");
        }
    }
    Ok(())
}
