use anyhow::anyhow;
use anyhow::Context;
use lazy_static::lazy_static;

/// download through download page
fn download_indirect<D: AsRef<std::path::Path>>(
    dir: D,
    status: &str,
    html: String,
) -> anyhow::Result<()> {
    let document = scraper::Html::parse_document(&html);
    let selector = scraper::Selector::parse("a").unwrap();
    let element = document
        .select(&selector)
        .find(|element| {
            matches!(
                element.text().next(),
                Some("Download Now") | Some("Download Specification ")
            )
        })
        .ok_or_else(|| anyhow!("can't find download button"))?;

    let url = element
        .value()
        .attr("href")
        .ok_or_else(|| anyhow!("download button has no URL"))?;

    download(dir, status, url)
}

fn content_disposition_to_filename(s: &str) -> anyhow::Result<&str> {
    lazy_static! {
        static ref RE: regex::Regex = regex::Regex::new("^attachment;filename=\"(.*)\"$").unwrap();
    }

    let captures = RE.captures(s).context("can't get captures")?;

    Ok(captures
        .get(1)
        .ok_or_else(|| anyhow!("can't find cpature group 1"))?
        .as_str())
}

fn download<D: AsRef<std::path::Path>>(dir: D, status: &str, url: &str) -> anyhow::Result<()> {
    let mut response = reqwest::blocking::get(url).context("can't download file")?;
    if !response.status().is_success() {
        return Err(anyhow!("failed with status {}", response.status()));
    }

    match response.headers().get("content-type") {
        Some(s) => match s.to_str() {
            Ok("application/pdf") => (),
            Ok("application/x-zip-compressed") => (),
            Ok("application/x-pdf") => (),
            Ok("application/unknown") => (),
            Ok(s) if s.starts_with("text/html") => {
                return download_indirect(
                    dir,
                    status,
                    response.text().context("can't download download page")?,
                )
                .context("download_indirect")
            }
            other => return Err(anyhow!("unsupported content-type `{:?}`", other)),
        },
        None => return Err(anyhow!("no content-type")),
    }

    let filename = match response.headers().get("content-disposition") {
        Some(s) => match s.to_str() {
            Ok(s) => match content_disposition_to_filename(s) {
                Ok(s) => s,
                other => return Err(anyhow!("unsupported content-disposition `{:?}`", other)),
            },
            other => return Err(anyhow!("unsupported content-disposition `{:?}`", other)),
        },
        None => return Err(anyhow!("no content-disposition")),
    };

    let statusdir = dir.as_ref().join(&status);
    let finalpath = statusdir.join(filename);

    if finalpath.exists() {
        log::info!("{:?} exists already, SKIP", finalpath);
        return Ok(());
    }

    std::fs::create_dir_all(&statusdir).context("can't create status dir")?;

    let mut temp = tempfile::Builder::new()
        .tempfile_in(statusdir)
        .context("can't create tempfile")?;
    std::io::copy(&mut response, &mut temp).context("can't download to tempfile")?;

    temp.persist(finalpath)
        .context("can't persist downloaded file")?;

    Ok(())
}

fn main() -> anyhow::Result<()> {
    env_logger::init();

    let dir = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow!("Missing filename"))?;
    let file_cached = std::path::Path::new("/tmp/btspecs.html");

    let body = if file_cached.exists() {
        String::from_utf8(std::fs::read(file_cached).context("can't read cache file")?)
            .context("cache file is not valid UTF8")?
    } else {
        let body = reqwest::blocking::get("https://www.bluetooth.com/specifications/specs/?status=all&show_latest_version=0&keyword=&filter=").context("can't download spec list")?.text().context("can't convert body to text")?;
        std::fs::write(file_cached, &body).context("can't write cache file")?;

        body
    };

    let document = scraper::Html::parse_document(&body);

    let selector = scraper::Selector::parse(r#"tr[class*="spec"]"#).unwrap();
    for element in document.select(&selector) {
        let selector = scraper::Selector::parse(r#"td[class="status"]"#).unwrap();
        let status = element
            .select(&selector)
            .next()
            .unwrap()
            .text()
            .next()
            .unwrap();
        if status.is_empty() {
            return Err(anyhow!("empty status"));
        }

        let url = if let Some(attr) = element
            .value()
            .attr("data-recommended")
            .map_or_else(|| None, |s| if s == "false" { None } else { Some(s) })
        {
            let mut v: serde_json::Value =
                serde_json::from_str(attr).context("can't parse data as json")?;
            match v.get_mut("url").unwrap().take() {
                serde_json::Value::String(s) => s,
                _ => return Err(anyhow!("non-string url")),
            }
        } else {
            let selector = scraper::Selector::parse(r#"a[href]"#).unwrap();
            if let Some(attr) = element.select(&selector).next() {
                attr.value().attr("href").unwrap().to_string()
            } else {
                log::warn!("no href: {:?}", element.value());
                continue;
            }
        };

        if url.is_empty() {
            return Err(anyhow!("empty url"));
        }

        log::debug!("{}, {}", status, url);

        if let Err(e) = download(&dir, status, &url) {
            log::error!("request failed for '{}': {:?}", url, e);
        }
    }

    Ok(())
}
