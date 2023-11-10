use reqwest::{Client, Error};
use serde::Deserialize;
use chrono::NaiveDate;

#[derive(Deserialize, Debug)]
pub struct EOLEntity {
    pub cycle: String,
    pub lts: bool,
    #[serde(rename(deserialize = "releaseDate"))]
    pub release_date: NaiveDate,
    pub latest: String,
    pub support: NaiveDate,
    pub eol: NaiveDate,
    #[serde(rename(deserialize = "latestReleaseDate"))]
    pub latest_release_date: Option<NaiveDate>,
}

pub async fn fetch_eol(product_name: &str) -> Result<Vec<EOLEntity>, Error> {
    let items = Client::new()
        .get(format!("https://endoflife.date/api/{}.json", product_name))
        .send()
        .await?
        .json::<Vec<EOLEntity>>()
        .await?;
    Ok(items)
}
