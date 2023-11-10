use azure_identity::AzureCliCredential;
use azure_mgmt_compute::models::os_disk::OsType;
use futures::stream::StreamExt;
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::Sender;
use std::fs::File;
use std::io::BufWriter;
use std::io::Write;
use tokio::sync::mpsc;
use tokio::sync::Mutex;
use xlsxwriter::prelude::*;
use paris::Logger;
use paris::error;

mod ubuntu;
mod centos;
mod windows;
mod eol;

use std::path::PathBuf;
use clap::Parser;

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
                if let Some(properties) = vm.properties {
                    let storage_profile = properties.storage_profile.unwrap();
                    let image_reference = storage_profile.image_reference.unwrap();
                    let os_disk = storage_profile.os_disk.unwrap();
                    let sku = image_reference.sku.unwrap();
                    
                    let resource_id = vm.resource.id.unwrap_or_default();
                    // info!("Found VM: {}", &resource_id);
                    let machine = VMResult {
                        id: resource_id,
                        subscription_id: subscription_id.clone(),
                        publisher: image_reference.publisher.unwrap_or_default(),
                        offer: image_reference.offer.unwrap_or_default(),
                        sku: sku.clone(),
                        version: image_reference.version.unwrap_or_default(),
                        exact_version: image_reference.exact_version.unwrap_or_default(),
                        os_type: os_disk.os_type.unwrap(),
                    };
                    let tx = tx.lock().await;
                    _ = tx.send(machine).await;
                } else {
                    error!("No properties found for VM {:?}", vm.resource.id);
                }
            }
        }
    })
    .await;
}

async fn write_to_excel(rx: &mut Receiver<VMResult>, file: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    let ubuntu_eol = ubuntu::list().await?;
    let centos_eol = centos::list().await?;
    let windows_eol = windows::list().await?;

    let workbook = Workbook::new_with_options(file.to_str().unwrap(), true, None, false)?;
    let mut sheet = workbook.add_worksheet(None)?;

    let header_format = Format::new().set_bold().set_bg_color(FormatColor::Gray).set_font_color(FormatColor::White).clone();
    let header_format = Some(&header_format);
    let eol_style = Format::new().set_bold().set_bg_color(FormatColor::Red).set_font_color(FormatColor::Black).clone();
    let eol_style = Some(&eol_style);
    let unknown_style = Format::new().set_bold().set_bg_color(FormatColor::Yellow).set_font_color(FormatColor::Black).clone();
    let unknown_style = Some(&unknown_style);
    let green_style = Format::new().set_bold().set_bg_color(FormatColor::Green).set_font_color(FormatColor::Black).clone();
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
            if vm.os_type == OsType::Linux && vm.offer.to_lowercase().contains("ubuntu") {
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

        let deprecated_sytle = {
            if version_info.1 == "EOL" {
                eol_style
            } else if version_info.1 == "Supported" {
                green_style
            } else {
                unknown_style
            }
        };

        sheet.write_string(row_idx, 0, &version_info.0, None)?;
        sheet.write_string(row_idx, 1, &version_info.1, deprecated_sytle)?;
        sheet.write_string(row_idx, 2, format!("{:?}", vm.os_type).as_str(), None)?;
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
            if vm.os_type == OsType::Linux && vm.offer.to_lowercase().contains("ubuntu") {
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

#[derive(Debug, Clone)]
pub struct VMResult {
    pub id: String,
    pub subscription_id: String,
    pub publisher: String,
    pub offer: String,
    pub sku: String,
    pub version: String,
    pub exact_version: String,
    pub os_type: OsType,
}

impl VMResult {
    fn csv_header_line() -> String {
        String::from("Deprecated;Version (detected);ID;OS;Subscription;Publisher;Offer;SKU;Version;Exact version\n")
    }
}
