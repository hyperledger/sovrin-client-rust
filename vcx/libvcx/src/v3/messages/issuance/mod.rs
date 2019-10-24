use v3::messages::MessageType;

pub mod credential;
pub mod credential_offer;
pub mod credential_proposal;
pub mod credential_request;

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct CredentialPreviewData {
    #[serde(rename="@type")]
    pub _type: MessageType,
    pub attributes: Vec<CredentialValue>
}

impl CredentialPreviewData {
    pub fn new() -> Self {
        unimplemented!()
//        CredentialPreviewData {
//            _type: "".to_string(),
//            attributes: vec![]
//        }
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "mime-type")]
pub enum CredentialValue {
    #[serde(rename="text/plain")]
    String(CredentialValueData)
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct CredentialValueData {
    pub name: String,
    pub value: String
}