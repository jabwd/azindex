mod eol_detection;
mod vmresult;

use azure_identity::AzureCliCredential;
use futures::stream::StreamExt;
use tokio::sync::mpsc::{Receiver, Sender};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use tokio::sync::{mpsc, Mutex};
use xlsxwriter::prelude::*;
use paris::{Logger, error};
use clap::Parser;

use vmresult::VMResult;
use eol_detection::{centos, windows, ubuntu, redhat};

#[derive(Parser, Debug)]
#[command(
    author="Antwan van Houdt",
    version="0.1.0",
    about="List and detect EOL VMs in an Azure tenant",
    long_about = None
)]
pub struct Cli {
    #[arg(short, long)]
    pub format: OutputType,
    pub out: PathBuf,
}

#[derive(Clone, Debug)]
pub enum OutputType {
    EXCEL,
    CSV,
    UNKNOWN,
}

impl From<String> for OutputType {
    fn from(other: String) -> Self {
        if other.to_lowercase() == "excel" {
            OutputType::EXCEL
        } else if other.to_lowercase() == "csv" {
            OutputType::CSV
        } else {
            OutputType::UNKNOWN
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Cli::parse();
    match args.format {
        OutputType::UNKNOWN => {
            error!("Unknown output format specified");
            return Ok(());
        },
        _ => {},
    };
    let mut log = Logger::new();
    log.info("Detecting credentials");
    

    let credential = std::sync::Arc::new(AzureCliCredential::new());
    let tenant = AzureCliCredential::get_tenant()?;
    log.loading(format!("Listing VMs in tenant {}", tenant));

    let subscription_client = azure_mgmt_subscription::Client::builder(credential.clone()).build();
    let client = azure_mgmt_compute::Client::builder(credential).build();
    let (tx, mut rx) = mpsc::channel::<VMResult>(32);

    tokio::spawn(async move {
        let tx = Mutex::new(tx);
        let subs = subscription_client
            .subscriptions_client()
            .list()
            .into_stream();
        subs.for_each_concurrent(10, |subs| async {
            if let Ok(subs) = subs {
                for sub in subs.value {
                    let sub_id = sub.subscription_id.unwrap_or_default();
                    // info!("> Listing VMs for {}", &sub_id);
                    list_vms(&sub_id, &client, &tx).await;
                }
            }
        }).await;
        log.done();
    });

    match args.format {
        OutputType::CSV => {
            write_to_csv(&mut rx, args.out).await?;
        },
        OutputType::EXCEL => {
            write_to_excel(&mut rx, args.out).await?;
        },
        _ => {}
    };

    
    let mut log = Logger::new();
    log.success("Done!");

    Ok(())
}

async fn list_vms(subscription_id: &String, client: &azure_mgmt_compute::Client, tx: &Mutex<Sender<VMResult>>) {
    let vms = client
        .virtual_machines_client()
        .list_all(subscription_id)
        .into_stream();
    vms.for_each_concurrent(10, |vms| async {
        if let Ok(vms) = vms {
            for vm in vms.value {
                let properties = match vm.properties {
                    Some(p) => p,
                    None => {
                        error!("No properties found for: {}", vm.resource.id.unwrap_or_default());
                        continue;
                    },
                };
                let storage_profile = match properties.storage_profile {
                    Some(p) => p,
                    None => {
                        error!("No storage profile found for: {}", vm.resource.id.unwrap_or_default());
                        continue;
                    },
                };
                let image_info = {
                    if let Some(r) = storage_profile.image_reference {
                        (r.sku.unwrap_or_default(), r.publisher.unwrap_or_default(), r.offer.unwrap_or_default(), r.version.unwrap_or_default(), r.exact_version.unwrap_or_default())
                    } else {
                        ("".to_string(), "".to_string(), "".to_string(), "".to_string(), "".to_string())
                    }
                };
                let os_disk = match storage_profile.os_disk {
                    Some(p) => p,
                    None => {
                        error!("No OS disk found for: {}", vm.resource.id.unwrap_or_default());
                        continue;
                    },
                };
                let sku = image_info.0;
                
                let resource_id = vm.resource.id.unwrap_or_default();
                // info!("Found VM: {}", &resource_id);
                let machine = VMResult {
                    id: resource_id,
                    subscription_id: subscription_id.clone(),
                    publisher: image_info.1,
                    offer: image_info.2,
                    sku: sku.clone(),
                    version: image_info.3,
                    exact_version: image_info.4,
                    os_type: os_disk.os_type,
                };
                let tx = tx.lock().await;
                _ = tx.send(machine).await;
            }
        }
    })
    .await;
}

async fn write_to_excel(rx: &mut Receiver<VMResult>, file: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    let ubuntu_eol = ubuntu::list().await?;
    let centos_eol = centos::list().await?;
    let windows_eol = windows::list().await?;
    let redhat_eol = redhat::list().await?;

    let workbook = Workbook::new_with_options(file.to_str().unwrap(), true, None, false)?;
    let mut sheet = workbook.add_worksheet(None)?;

    let header_format = Format::new().set_bold().set_bg_color(FormatColor::Gray).set_font_color(FormatColor::White).set_border_bottom(FormatBorder::Medium).clone();
    let header_format = Some(&header_format);
    let eol_style = Format::new().set_bold().set_bg_color(FormatColor::Custom(0xF5_CA_C9)).set_font_color(FormatColor::Custom(0x8D_20_12)).clone();
    let eol_style = Some(&eol_style);
    let unknown_style = Format::new().set_bold().set_bg_color(FormatColor::Custom(0xFA_EC_A2)).set_font_color(FormatColor::Custom(0x915C17)).clone();
    let unknown_style = Some(&unknown_style);
    let green_style = Format::new().set_bold().set_bg_color(FormatColor::Custom(0xCF_ED_CF)).set_font_color(FormatColor::Custom(0x295F10)).clone();
    let green_style = Some(&green_style);

    sheet.write_string(0, 0, "Detected version", header_format)?;
    sheet.write_string(0, 1, "Deprecated", header_format)?;
    sheet.write_string(0, 2, "OS", header_format)?;
    sheet.write_string(0, 3, "Subscription", header_format)?;
    sheet.write_string(0, 4, "Offer", header_format)?;
    sheet.write_string(0, 5, "SKU", header_format)?;
    sheet.write_string(0, 6, "Version", header_format)?;
    sheet.write_string(0, 7, "Version exact", header_format)?;
    sheet.write_string(0, 8, "Publisher", header_format)?;
    sheet.write_string(0, 9, "Resource ID", header_format)?;

    let mut row_idx = 1;
    while let Some(vm) = rx.recv().await {
        let version_info: (String, String) = {
            if vm.offer.to_lowercase().contains("ubuntu") {
                let version = ubuntu::parse_azure_version(&vm.sku);
                let is_outdated = ubuntu::is_outdated(&vm, &ubuntu_eol);
                (version.unwrap_or_default(), is_outdated)
            } else if vm.offer.to_lowercase().contains("centos") {
                let version = centos::parse_azure_version(&vm.sku);
                let is_outdated = centos::is_outdated(&vm, &centos_eol);
                (version.unwrap_or_default(), is_outdated)
            } else if vm.offer.to_lowercase().contains("windows") {
                let version = windows::parse_azure_version(&vm.sku);
                let is_outdated = windows::is_outdated(&vm, &windows_eol);
                (version.unwrap_or_default(), is_outdated)
            } else if vm.offer.to_lowercase().contains("rhel") {
                let version = redhat::parse_azure_version(&vm.sku);
                let is_outdated = redhat::is_outdated(&vm, &redhat_eol);
                (version.unwrap_or_default(), is_outdated)
            } else {
                (String::from(""), String::from("--"))
            }
        };

        let deprecated_sytle = {
            if version_info.1 == "EOL" {
                eol_style
            } else if version_info.1 == "Supported" {
                green_style
            } else {
                unknown_style
            }
        };

        let os_type = {
            if let Some(os_type) = vm.os_type {
                format!("{:?}", os_type)
            } else {
                String::from("--")
            }
        };

        sheet.write_string(row_idx, 0, &version_info.0, None)?;
        sheet.write_string(row_idx, 1, &version_info.1, deprecated_sytle)?;
        sheet.write_string(row_idx, 2, &os_type.as_str(), None)?;
        sheet.write_string(row_idx, 3, &vm.subscription_id, None)?;
        sheet.write_string(row_idx, 4, &vm.offer, None)?;
        sheet.write_string(row_idx, 5, &vm.sku, None)?;
        sheet.write_string(row_idx, 6, &vm.version, None)?;
        sheet.write_string(row_idx, 7, &vm.exact_version, None)?;
        sheet.write_string(row_idx, 8, &vm.publisher, None)?;
        sheet.write_string(row_idx, 9, &vm.id, None)?;

        row_idx += 1;
    }
    workbook.close()?;

    Ok(())
}

async fn write_to_csv(rx: &mut Receiver<VMResult>, file: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    let file = File::create(file)?;
    let mut f = BufWriter::new(file);
    f.write(VMResult::csv_header_line().as_bytes())?;

    let ubuntu_eol = ubuntu::list().await?;
    let centos_eol = centos::list().await?;
    let windows_eol = windows::list().await?;

    while let Some(vm) = rx.recv().await {
        let version_info: (String, String) = {
            if vm.offer.to_lowercase().contains("ubuntu") {
                let version = ubuntu::parse_azure_version(&vm.sku);
                let is_outdated = ubuntu::is_outdated(&vm, &ubuntu_eol);
                (version.unwrap_or_default(), is_outdated)
            } else if vm.offer.to_lowercase().contains("centos") {
                let version = centos::parse_azure_version(&vm.sku);
                let is_outdated = centos::is_outdated(&vm, &centos_eol);
                (version.unwrap_or_default(), is_outdated)
            } else if vm.offer.to_lowercase().contains("windows") {
                let version = windows::parse_azure_version(&vm.sku);
                let is_outdated = windows::is_outdated(&vm, &windows_eol);
                (version.unwrap_or_default(), is_outdated)
            } else {
                (String::from(""), String::from("--"))
            }
        };

        let line = format!(
            "{};{};{};{:?};{};{};{};{};{};{}\n",
            version_info.0, version_info.1, vm.id, vm.os_type, vm.subscription_id, vm.publisher, vm.offer, vm.sku, vm.version, vm.exact_version
        );
        f.write(line.as_bytes())?;
    }

    Ok(())
}
