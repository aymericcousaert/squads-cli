use serde::{Deserialize, Serialize};

/// SharePoint site
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Site {
    pub id: String,
    pub display_name: Option<String>,
    pub name: Option<String>,
    pub web_url: Option<String>,
    pub description: Option<String>,
}

/// Sites list response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sites {
    #[serde(rename = "@odata.context")]
    pub context: Option<String>,
    pub value: Vec<Site>,
}

/// Drive (document library)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Drive {
    pub id: String,
    pub name: Option<String>,
    pub drive_type: Option<String>,
    pub web_url: Option<String>,
    pub description: Option<String>,
}

/// Drives list response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Drives {
    #[serde(rename = "@odata.context")]
    pub context: Option<String>,
    pub value: Vec<Drive>,
}

/// Drive item (file or folder)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveItem {
    pub id: String,
    pub name: Option<String>,
    pub web_url: Option<String>,
    pub size: Option<i64>,
    pub last_modified_date_time: Option<String>,
    pub folder: Option<FolderFacet>,
    pub file: Option<FileFacet>,
    pub parent_reference: Option<ParentReference>,
}

/// Folder facet (present if item is a folder)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderFacet {
    pub child_count: Option<i32>,
}

/// File facet (present if item is a file)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileFacet {
    pub mime_type: Option<String>,
}

/// Parent reference
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParentReference {
    pub drive_id: Option<String>,
    pub id: Option<String>,
    pub path: Option<String>,
}

/// Drive items list response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriveItems {
    #[serde(rename = "@odata.context")]
    pub context: Option<String>,
    #[serde(rename = "@odata.nextLink")]
    pub next_link: Option<String>,
    pub value: Vec<DriveItem>,
}

/// Excel worksheet
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Worksheet {
    pub id: Option<String>,
    pub name: String,
    pub position: Option<i32>,
    pub visibility: Option<String>,
}

/// Worksheets list response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Worksheets {
    #[serde(rename = "@odata.context")]
    pub context: Option<String>,
    pub value: Vec<Worksheet>,
}

/// Excel range
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExcelRange {
    pub address: Option<String>,
    pub address_local: Option<String>,
    pub cell_count: Option<i64>,
    pub column_count: Option<i32>,
    pub row_count: Option<i32>,
    pub values: Option<Vec<Vec<serde_json::Value>>>,
    pub text: Option<Vec<Vec<serde_json::Value>>>,
    pub formulas: Option<Vec<Vec<serde_json::Value>>>,
    pub number_format: Option<Vec<Vec<serde_json::Value>>>,
}

/// Request body for updating a range
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateRangeRequest {
    pub values: Vec<Vec<serde_json::Value>>,
}

/// Excel table
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExcelTable {
    pub id: Option<String>,
    pub name: Option<String>,
    pub show_headers: Option<bool>,
    pub show_totals: Option<bool>,
}

/// Tables list response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExcelTables {
    #[serde(rename = "@odata.context")]
    pub context: Option<String>,
    pub value: Vec<ExcelTable>,
}

/// Request body for adding rows to a table
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddTableRowsRequest {
    pub values: Vec<Vec<serde_json::Value>>,
}
