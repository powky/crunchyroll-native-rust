//! The home feed and the lists behind it. The feed is a list of row recipes ([`RowSpec`]) that comes in one request;
//! each row's cards are a request of their own, made when the row is about to show.

use anyhow::Result;
use crunchyroll_rs::feed::HomeFeed;
use crunchyroll_rs::list::WatchlistOptions;
use crunchyroll_rs::search::BrowseOptions;
use crunchyroll_rs::{Crunchyroll, MediaCollection, Series};
use futures_util::StreamExt;
use futures_util::future::join_all;

use crate::model::{Card, Row};

/// Cards per row: about four screens of a row, more than anyone scrolls before opening something.
const ROW_SIZE: usize = 24;
/// How many feed items to read: Crunchyroll's home feed runs to a few dozen.
const FEED_SIZE: usize = 40;

/// A home row, before its cards are loaded.
#[derive(Clone, Debug)]
pub enum RowSpec {
    ContinueWatching,
    Watchlist,
    Recommendations,
    /// A curated row: a title and the series in it.
    Series { title: String, ids: Vec<String> },
    /// A catalog query (new, popular, a genre); Crunchyroll gives these rows no title of their own.
    Browse { options: BrowseOptions },
    SimilarTo { title: String, series_id: String },
    /// The carousel at the top, already loaded with the feed.
    Featured(Vec<Card>),
}

/// The home feed's rows, in Crunchyroll's order, with "continue watching" moved first or second.
pub async fn home(crunchy: &Crunchyroll, continue_first: bool) -> Result<Vec<RowSpec>> {
    let mut specs = Vec::new();
    let mut featured = Vec::new();
    let mut feed = crunchy.home_feed().take(FEED_SIZE);
    while let Some(item) = feed.next().await {
        let Ok(item) = item else { continue };
        match item {
            HomeFeed::CarouselFeed(carousel) => {
                featured.extend(carousel.iter().filter_map(|slide| slide.panel.as_ref().and_then(Card::from_collection)));
            }
            HomeFeed::Series(series) => {
                featured.extend(Card::from_collection(&MediaCollection::Series(series)));
            }
            HomeFeed::History => specs.push(RowSpec::ContinueWatching),
            HomeFeed::Watchlist => specs.push(RowSpec::Watchlist),
            HomeFeed::Recommendation => specs.push(RowSpec::Recommendations),
            HomeFeed::SeriesFeed(row) if !row.ids.is_empty() => {
                specs.push(RowSpec::Series { title: row.title, ids: row.ids });
            }
            HomeFeed::Browse(options) => specs.push(RowSpec::Browse { options }),
            HomeFeed::SimilarTo(similar) => {
                specs.push(RowSpec::SimilarTo { title: similar.title, series_id: similar.similar_id });
            }
            _ => {}
        }
    }
    if !featured.is_empty() {
        specs.insert(0, RowSpec::Featured(featured));
    }
    if let Some(at) = specs.iter().position(|spec| matches!(spec, RowSpec::ContinueWatching)) {
        let history = specs.remove(at);
        let target = if continue_first { 0 } else { specs.len().min(1) };
        specs.insert(target, history);
    }
    Ok(specs)
}

/// A row's cards.
pub async fn load_row(crunchy: &Crunchyroll, spec: &RowSpec) -> Result<Vec<Card>> {
    Ok(match spec {
        RowSpec::Featured(cards) => cards.clone(),
        RowSpec::ContinueWatching => continue_watching(crunchy).await?,
        RowSpec::Watchlist => crunchy
            .watchlist(WatchlistOptions::default())
            .await?
            .iter()
            .filter_map(|entry| Card::from_collection(&entry.panel))
            .take(ROW_SIZE)
            .collect(),
        RowSpec::Recommendations => crunchy
            .recommendations()
            .take(ROW_SIZE)
            .filter_map(|item| async move { item.ok().as_ref().and_then(Card::from_collection) })
            .collect()
            .await,
        RowSpec::Series { ids, .. } => {
            let series = join_all(ids.iter().take(ROW_SIZE).map(|id| crunchy.media_from_id::<Series>(id))).await;
            series.into_iter().flatten().filter_map(|series| Card::from_collection(&MediaCollection::Series(series))).collect()
        }
        RowSpec::Browse { options } => crunchy
            .browse(options.clone())
            .take(ROW_SIZE)
            .filter_map(|item| async move { item.ok().map(Into::into).as_ref().and_then(Card::from_collection) })
            .collect()
            .await,
        RowSpec::SimilarTo { series_id, .. } => {
            let series: Series = crunchy.media_from_id(series_id).await?;
            series
                .similar()
                .take(ROW_SIZE)
                .filter_map(|item| async move { item.ok().map(Into::into).as_ref().and_then(Card::from_collection) })
                .collect()
                .await
        }
    })
}

/// Episodes in progress, newest first, one per series (the latest one watched).
pub async fn continue_watching(crunchy: &Crunchyroll) -> Result<Vec<Card>> {
    let mut cards: Vec<Card> = Vec::new();
    let mut history = crunchy.watch_history().take(ROW_SIZE * 3);
    while let Some(entry) = history.next().await {
        let Ok(entry) = entry else { continue };
        if entry.fully_watched {
            continue;
        }
        let Some(card) = entry.panel.as_ref().and_then(Card::from_collection) else { continue };
        if card.series_id.is_some() && cards.iter().any(|seen| seen.series_id == card.series_id) {
            continue;
        }
        cards.push(card.with_playhead(entry.playhead, entry.fully_watched));
        if cards.len() == ROW_SIZE {
            break;
        }
    }
    Ok(cards)
}

/// A finished row, for callers that want title and cards together.
pub async fn row(crunchy: &Crunchyroll, spec: &RowSpec, title: String) -> Result<Row> {
    Ok(Row { title, cards: load_row(crunchy, spec).await? })
}
