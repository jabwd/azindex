use crate::VMResult;
use crate::eol_detection::eol::{fetch_eol, EOLEntity};
use reqwest::Error;

pub async fn list() -> Result<Vec<EOLEntity>, Error> {
    fetch_eol("ubuntu").await
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
            if item.eol < now {
                return "EOL".to_string();
            } else if item.eol > now {
                return "Supported".to_string();
            }
            println!("Item matches: {:#?}", item);
        }
    }

    "--".to_string()
}

pub fn parse_azure_version(az_version: &String) -> Option<String> {
    // Examples:
    // 18.04-LTS, 20_04-lts-gen2
    let parts: Vec<&str> = az_version.split("-").collect();
    if parts.is_empty() {
        return None;
    }
    let first = parts[0];
    let parts: Vec<&str> = first.split("_").collect();
    if parts.len() == 2 {
        let version = format!("{}.{}", parts[0], parts[1]);
        return Some(version);
    }
    Some(String::from(first))
}
