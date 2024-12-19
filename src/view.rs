use {serde::Deserialize, std::collections::HashMap};

#[derive(Debug, Clone)]
pub struct ParamView {
    name: String,
    sort_order: isize,
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
    sort_order: OSCQueryItem<isize>,
}

impl ParamView {
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
                            .map(|s| {
                                let mut split = s.split(":");
                                if let Some(Ok(instance)) = split.next().map(|p| p.parse::<usize>())
                                {
                                    if let Some(Ok(param)) =
                                        split.next().map(|p| p.parse::<usize>())
                                    {
                                        return Ok((instance, param));
                                    }
                                }
                                Err(())
                            })
                            .collect();
                        if let Ok(params) = params {
                            views.push(ParamView {
                                index,
                                name: value.name.value.clone(),
                                params,
                                sort_order: value.sort_order.value,
                            })
                        }
                    }
                }
            }
        }
        views
    }
}
