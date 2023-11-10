use azure_mgmt_compute::models::os_disk::OsType;

#[derive(Debug, Clone)]
pub struct VMResult {
    pub id: String,
    pub subscription_id: String,
    pub publisher: String,
    pub offer: String,
    pub sku: String,
    pub version: String,
    pub exact_version: String,
    pub os_type: Option<OsType>,
}

impl VMResult {
    pub fn csv_header_line() -> String {
        String::from("Deprecated;Version (detected);ID;OS;Subscription;Publisher;Offer;SKU;Version;Exact version\n")
    }
}
