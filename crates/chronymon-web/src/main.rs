//! chronymon-web: a browser view of chronymon's database.
//!
//! Serves pages and some JSON.  The pages do the drawing, because a
//! browser already knows how to draw.  The server reads the database
//! the daemon writes and nothing else: no socket to chronyd, so no way
//! to send it a command from here, which is deliberate.  A page on a
//! TCP port has none of the authorization chronyd's socket does.

use std::path::PathBuf;
use std::sync::LazyLock;

use anyhow::Result;
use chronymon_http::Response;
use clap::Parser;
use serde_json::Value;
use serde_json::json;

mod api;

/// The pages, built in rather than read from disk: one file to install,
/// and a running server cannot be made to serve something else by
/// writing to a directory it happens to have.
const LIVE: &str = include_str!("index.html");
/// Every source, a table and their measurements overlaid.
const SOURCES: &str = include_str!("sources.html");
/// One source's measurements and regression.
const SOURCE: &str = include_str!("source.html");
/// Each refclock's samples, from chronyd's log.
const REFCLOCKS: &str = include_str!("refclocks.html");
/// tempcomp's temperature and the clock's frequency against it.
const TEMPERATURE: &str = include_str!("temperature.html");
/// The events over a range.
const JOURNAL: &str = include_str!("journal.html");
/// Percentiles and histograms over a range.
const DISTRIBUTION: &str = include_str!("distribution.html");
/// One screen for an unattended display: the clock's state in large
/// type and its history, with nothing to click; or other pages in turn.
const KIOSK: &str = include_str!("kiosk.html");

/// The kiosk as served: its stylesheet and this server's scripts
/// inlined, so a reload paints from one answer and waits on no other
/// request, which a signage player shows as a white screen.
static KIOSK_SERVED: LazyLock<String> = LazyLock::new(|| inlined(KIOSK));

/// What `inlined` replaces: each tag, and the element and text it
/// becomes.
const INLINED: [(&str, &str, &str); 3] = [
    (
        "<link rel=\"stylesheet\" href=\"/style.css\">",
        "style",
        STYLE,
    ),
    ("<script src=\"/common.js\"></script>", "script", COMMON),
    ("<script src=\"/charts.js\"></script>", "script", CHARTS),
];

/// `page` with the files `INLINED` names written into it.
fn inlined(page: &str) -> String {
    INLINED
        .iter()
        .fold(page.to_owned(), |page, (tag, element, text)| {
            page.replace(tag, &format!("<{element}>\n{text}</{element}>"))
        })
}

/// Shared by every page.
const STYLE: &str = include_str!("style.css");
/// The status strip, the range control and the lifecycle.
const COMMON: &str = include_str!("common.js");
/// Axes, stacks and event marks.
const CHARTS: &str = include_str!("charts.js");

const HTML: &str = "text/html; charset=utf-8";
const CSS: &str = "text/css; charset=utf-8";
const JAVASCRIPT: &str = "text/javascript; charset=utf-8";
const JSON: &str = "application/json; charset=utf-8";

/// Options, each also settable in the environment, which is how
/// `/etc/default/chronymon-web` sets them.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Cli {
    /// The database chronymond writes.  Opened to read, never created;
    /// this user needs to be in the database's group.
    #[arg(
        long,
        env = "CHRONYMON_WEB_DATABASE",
        default_value = "/var/lib/chronymon/chronymon.sqlite"
    )]
    database: PathBuf,

    /// Address to serve on.
    ///
    /// Localhost by default.  The pages have no authentication of their
    /// own, so reaching them from elsewhere should be a deliberate
    /// change here (0.0.0.0:8323) or an SSH tunnel, not an accident.
    #[arg(long, env = "CHRONYMON_WEB_LISTEN", default_value = "127.0.0.1:8323")]
    listen: String,
}

/// The first year of the project's copyright.
const COPYRIGHT_FROM: u16 = 2026;

/// What every page's colophon shows, from the package metadata.
fn about(cli: &Cli) -> Value {
    let holders: Vec<&str> = env!("CARGO_PKG_AUTHORS")
        .split(':')
        .map(|author| author.split(" <").next().unwrap_or(author).trim())
        .collect();
    json!({
        "copyright": format!("{COPYRIGHT_FROM} {}", holders.join(", ")),
        "license": env!("CARGO_PKG_LICENSE"),
        "version": env!("CARGO_PKG_VERSION"),
        "repository": env!("CARGO_PKG_REPOSITORY"),
        "database": cli.database.display().to_string(),
    })
}

/// Wrap a result as JSON, reporting a failure as data rather than as an
/// HTTP error: the page can then say what went wrong in the place the
/// value would have been, instead of silently showing nothing.
fn json(result: Result<Value>) -> Response {
    let value = match result {
        Ok(value) => value,
        Err(e) => json!({ "error": format!("{e:#}") }),
    };
    Response::ok(JSON, value.to_string())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    eprintln!(
        "chronymon-web: version {}, serving http://{}/ from {}",
        env!("CARGO_PKG_VERSION"),
        cli.listen,
        cli.database.display()
    );
    let listen = cli.listen.clone();
    chronymon_http::serve(&listen, move |target| {
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        let database = &cli.database;
        match path {
            "/" => Response::ok(HTML, LIVE.to_owned()),
            "/sources" => Response::ok(HTML, SOURCES.to_owned()),
            "/source" => Response::ok(HTML, SOURCE.to_owned()),
            "/refclocks" => Response::ok(HTML, REFCLOCKS.to_owned()),
            "/temperature" => Response::ok(HTML, TEMPERATURE.to_owned()),
            "/journal" => Response::ok(HTML, JOURNAL.to_owned()),
            "/distribution" => Response::ok(HTML, DISTRIBUTION.to_owned()),
            "/kiosk" => Response::ok(HTML, KIOSK_SERVED.clone()),
            "/style.css" => Response::ok(CSS, STYLE.to_owned()),
            "/common.js" => Response::ok(JAVASCRIPT, COMMON.to_owned()),
            "/charts.js" => Response::ok(JAVASCRIPT, CHARTS.to_owned()),
            "/api/about" => json(Ok(about(&cli))),
            "/api/status" => json(api::status(database)),
            "/api/history" => json(api::history(database, query)),
            "/api/overlay" => json(api::overlay(database, query)),
            "/api/sources" => json(api::sources(database)),
            "/api/events" => json(api::events(database, query)),
            "/api/keys" => json(api::keys(database, query)),
            "/api/distribution" => json(api::distribution(database, query)),
            _ => Response::not_found(),
        }
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::CHARTS;
    use super::COMMON;
    use super::INLINED;
    use super::KIOSK;
    use super::STYLE;
    use super::inlined;

    /// The kiosk is served whole: every file it would ask this server
    /// for is in it, and nothing an inlined file says ends its element
    /// early.
    #[test]
    fn the_kiosk_is_served_with_its_files_inlined() {
        let served = inlined(KIOSK);
        for (tag, element, text) in INLINED {
            assert!(KIOSK.contains(tag), "{tag} is not in kiosk.html");
            assert!(!served.contains(tag));
            assert!(!text.contains(&format!("</{element}")));
        }
        for text in [STYLE, COMMON, CHARTS] {
            assert!(served.contains(text));
        }
    }
}
