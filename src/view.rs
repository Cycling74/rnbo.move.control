use {serde::Deserialize, std::collections::HashMap};

#[derive(Debug, Clone)]
pub struct ParamView {
    name: String,
    params: Vec<(usize, usize)>,
    index: usize,
}

#[derive(Deserialize, Debug, Default)]
struct OSCQueryItem<T> {
    #[serde(rename = "VALUE")]
    value: T,
    //XXX plenty of other fields but we don't need them right now
}

#[derive(Deserialize, Debug, Default)]
struct OSCQueryContents<T> {
    #[serde(rename = "CONTENTS")]
    contents: Option<T>,
}

#[derive(Deserialize, Debug, Default)]
struct ParamViewListItem {
    name: OSCQueryItem<String>,
    params: OSCQueryItem<Vec<String>>,
}

impl ParamView {
    pub fn new(name: String, params: Vec<(usize, usize)>, index: usize) -> Self {
        Self {
            name,
            params,
            index,
        }
    }
    pub fn index(&self) -> usize {
        self.index
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn set_name(&mut self, name: String) {
        self.name = name;
    }

    pub fn params(&self) -> &Vec<(usize, usize)> {
        &self.params
    }

    pub fn set_params(&mut self, params: Vec<(usize, usize)>) {
        self.params = params;
    }

    pub fn parse_param_s(v: &str) -> Result<(usize, usize), ()> {
        let mut split = v.split(":");
        if let Some(Ok(instance)) = split.next().map(|p| p.parse::<usize>()) {
            if let Some(Ok(param)) = split.next().map(|p| p.parse::<usize>()) {
                return Ok((instance, param));
            }
        }
        Err(())
    }

    pub fn parse_all(json: &serde_json::Value) -> Vec<Self> {
        let mut views = Vec::new();
        let parsed: Result<
            OSCQueryContents<HashMap<String, OSCQueryContents<ParamViewListItem>>>,
            _,
        > = serde_json::from_value(json.clone());
        if let Ok(parsed) = parsed {
            if let Some(content) = parsed.contents {
                for (key, value) in content.iter() {
                    if let Some(value) = value.contents.as_ref() {
                        let index = key.parse::<usize>().unwrap();
                        let params: Result<Vec<(usize, usize)>, _> = value
                            .params
                            .value
                            .iter()
                            .map(|s| Self::parse_param_s(s))
                            .collect();
                        if let Ok(params) = params {
                            views.push(ParamView {
                                index,
                                name: value.name.value.clone(),
                                params,
                            })
                        }
                    }
                }
            }
        }
        views
    }
}
