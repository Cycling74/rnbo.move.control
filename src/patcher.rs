use {crate::param::Param, std::collections::HashMap};

#[derive(Debug)]
pub struct PatcherInst {
    index: usize,
    name: String,
    params: Vec<Param>,
}

impl PatcherInst {
    pub fn parse(index: usize, json: &serde_json::Value) -> Option<Self> {
        let contents = json.get("CONTENTS")?.as_object()?;
        let name = contents
            .get("name")?
            .as_object()?
            .get("VALUE")?
            .as_str()?
            .to_string();
        let params = Param::parse_all(contents.get("params")?)?;
        Some(PatcherInst {
            index,
            name,
            params,
        })
    }

    pub fn parse_all(json: &serde_json::Value) -> Option<HashMap<usize, Self>> {
        let mut inst = HashMap::new();
        let contents = json.get("CONTENTS")?.as_object()?;

        for (key, value) in contents.iter() {
            if let Ok(index) = key.parse::<usize>() {
                inst.insert(index, Self::parse(index, value)?);
            }
        }

        Some(inst)
    }
}
