//! Tries the backend: `cli code` asks for a sign-in code (and signs nothing in); the other commands use the session
//! saved in `CRUNCHY_DATA_DIR` and print titles and counts, never tokens. They rotate the refresh token and save it:
//! on the TV run them as the app's own user (not root) with the app closed, or the app can't read its session back.

use crunchy_core::auth;
use crunchy_core::content::{Audio, Browse, Ctx, Sort};
use crunchy_core::crunchyroll_rs::Locale;
use crunchy_core::model::Card;
use crunchy_core::platform::Platform;
use crunchy_core::session::Session;
use crunchy_core::store::Store;

fn line(card: &Card) -> String {
    let episode = card.episode.as_ref().map(|e| format!(" T{} E{} {} ({}s)", e.season, e.number, e.name, e.duration_secs)).unwrap_or_default();
    format!(
        "{:?} {} {}{} | {:?} {:?}y {:?}s {:?}e {:?} sub={} dub={} ★{:?} prog={:?} watched={} img={} poster={}",
        card.kind,
        card.id,
        card.title,
        episode,
        card.maturity,
        card.year,
        card.season_count,
        card.episode_count,
        card.date,
        card.subbed,
        card.dubbed,
        card.rating,
        card.progress,
        card.watched,
        card.image.is_some(),
        card.poster.is_some()
    )
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |n: usize| args.get(n).cloned().unwrap_or_default();
    if arg(0) == "code" {
        let dir = std::env::temp_dir().join("crunchy-cli");
        let platform = Platform::load(&dir, false).await?;
        let client = platform.http_client()?;
        let code = auth::request_code(&client, &platform).await?;
        println!("code {} for {}, every {:?}, expires in {:?}", code.user_code, auth::ACTIVATE_URL, code.interval, code.expires_in);
        return Ok(());
    }
    let dir = std::path::PathBuf::from(std::env::var("CRUNCHY_DATA_DIR")?);
    let platform = Platform::load(&dir, false).await?;
    let session = Session::restore(Store::new(&dir), platform, Locale::es_419).await?.ok_or_else(|| anyhow::anyhow!("not signed in"))?;
    let ctx = Ctx::new(session.crunchy.clone(), "es-419".to_string()).await?;
    match arg(0).as_str() {
        "show" => {
            let show = ctx.show(&arg(1)).await?;
            println!("{}\nlogo={:?}\nhero={:?}\nwatchlist={} mine={:?}", line(&show.card), show.logo, show.hero, show.in_watchlist, show.my_rating);
            println!("up next: {:?}", show.up_next.as_ref().map(line));
            for season in &show.seasons {
                println!("season {} {} '{}' {} eps", season.id, season.display_number, season.title, season.episode_count);
            }
        }
        "episodes" => ctx.episodes(&arg(1)).await?.iter().for_each(|card| println!("{}", line(card))),
        "movies" => ctx.movies(&arg(1)).await?.iter().for_each(|card| println!("{}", line(card))),
        "browse" => {
            let browse = Browse { media: Some("series"), sort: Sort::Popularity, audio: Audio::All, ..Default::default() };
            let page = ctx.browse(&browse, 0, 5).await?;
            println!("total {}", page.total);
            page.cards.iter().for_each(|card| println!("{}", line(card)));
        }
        "search" => ctx.search(&arg(1), 6).await?.iter().for_each(|card| println!("{}", line(card))),
        "categories" => ctx.categories().await?.iter().for_each(|c| println!("{} {}", c.id, c.title)),
        "tags" => ctx.seasonal_tags().await?.iter().take(6).for_each(|c| println!("{} {}", c.id, c.title)),
        "watchlist" => {
            let page = ctx.watchlist(0, 5).await?;
            println!("total {}", page.total);
            page.cards.iter().for_each(|card| println!("{}", line(card)));
        }
        "history" => {
            let page = ctx.history(1, 5).await?;
            println!("total {}", page.total);
            page.cards.iter().for_each(|card| println!("{}", line(card)));
        }
        "lists" => {
            for list in ctx.custom_lists().await? {
                println!("{} {} {:?}", list.id, list.title, list.count);
                ctx.custom_list(&list.id).await?.cards.iter().take(3).for_each(|card| println!("  {}", line(card)));
            }
        }
        "similar" => ctx.similar(&arg(1), 5).await?.iter().for_each(|card| println!("{}", line(card))),
        "rating" => println!("{:?}", ctx.my_rating(&arg(1), crunchy_core::model::Kind::Series).await?),
        "unrate" => {
            ctx.rate(&arg(1), crunchy_core::model::Kind::Series, None).await?;
            println!("removed; now {:?}", ctx.my_rating(&arg(1), crunchy_core::model::Kind::Series).await?);
        }
        "raw" => {
            let value = ctx.raw(&arg(1).replace("{acct}", &ctx.account), &[]).await?;
            let text = serde_json::to_string_pretty(&value)?;
            println!("{}", text.chars().take(arg(2).parse().unwrap_or(3000)).collect::<String>());
        }
        _ => eprintln!("usage: cli code | show ID | episodes SEASON | movies LISTING | browse | search TEXT | categories | tags | watchlist | history | lists | similar ID | raw PATH [CHARS]"),
    }
    Ok(())
}
