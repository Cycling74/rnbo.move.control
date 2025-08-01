use serde::Deserialize;

#[derive(Deserialize, Debug, Default)]
pub struct OSCQueryItem<T> {
    #[serde(rename = "VALUE")]
    pub value: T,
    //XXX plenty of other fields but we don't need them right now
}

#[derive(Deserialize, Debug, Default)]
pub struct OSCQueryContents<T> {
    #[serde(rename = "CONTENTS")]
    pub contents: Option<T>,
}
