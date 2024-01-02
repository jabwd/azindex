use crate::eol_detection::eol::{fetch_eol, EOLEntity};
use crate::VMResult;
use chrono::Months;
use reqwest::Error;

pub async fn list() -> Result<Vec<EOLEntity>, Error> {
    fetch_eol("redhat").await
}

pub fn is_outdated(vm: &VMResult, eol_list: &Vec<EOLEntity>) -> String {
    let version = match parse_azure_version(&vm.sku) {
        Some(v) => v,
        None => {
            eprintln!("[ ERROR ] Parsing azure version falied for {:#?}", vm);
            return "--".to_string();
        }
    };
    for item in eol_list {
        if item.cycle == version {
            let now = chrono::Utc::now().date_naive();
            let future_eol = chrono::Utc::now()
                .checked_add_months(Months::new(12))
                .unwrap()
                .date_naive();
            if item.eol < now {
                return "EOL".to_string();
            } else if item.eol > now {
                if item.eol < future_eol {
                    return format!("Ending {}", item.eol);
                }
                return "Supported".to_string();
            }
            println!("Item matches: {:#?}", item);
        }
    }

    "--".to_string()
}

pub fn parse_azure_version(az_version: &String) -> Option<String> {
    let parts: Vec<&str> = az_version.split(".").collect();
    if parts.len() < 2 {
        let parts: Vec<&str> = az_version.split("-").collect();
        if parts.is_empty() {
            return None;
        }
        return Some(parts[0].to_string());
    }
    return Some(parts[0].to_string());
}
