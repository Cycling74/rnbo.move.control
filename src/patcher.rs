use {crate::param::Param, std::collections::HashMap};

#[derive(Debug)]
pub struct PatcherInst {
    index: usize,
    name: String,
    params: Vec<Param>,
    presets: Vec<String>,
    data_refs: HashMap<String, Option<String>>,
}

fn parse_presets(contents: &serde_json::Map<String, serde_json::Value>) -> Option<Vec<String>> {
    let mut presets = Vec::new();
    for e in contents
        .get("presets")?
        .as_object()?
        .get("CONTENTS")?
        .as_object()?
        .get("entries")?
        .as_object()?
        .get("VALUE")?
        .as_array()?
        .iter()
    {
        presets.push(e.as_str()?.to_string());
    }

    Some(presets)
}

fn parse_datarefs(
    contents: &serde_json::Map<String, serde_json::Value>,
) -> Option<HashMap<String, Option<String>>> {
    let mut data_refs = HashMap::new();

    for (name, body) in contents
        .get("data_refs")?
        .as_object()?
        .get("CONTENTS")?
        .as_object()?
        .iter()
    {
        let value = body.get("VALUE")?.as_str()?;
        let value = if value.len() > 0 {
            Some(value.to_string())
        } else {
            None
        };
        data_refs.insert(name.clone(), value);
    }

    Some(data_refs)
}

impl PatcherInst {
    pub fn index(&self) -> usize {
        self.index
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn params(&self) -> &Vec<Param> {
        &self.params
    }

    pub fn params_mut(&mut self) -> &mut Vec<Param> {
        &mut self.params
    }

    pub fn data_refs(&self) -> &HashMap<String, Option<String>> {
        &self.data_refs
    }

    pub fn data_refs_mut(&mut self) -> &mut HashMap<String, Option<String>> {
        &mut self.data_refs
    }

    pub fn update_param_f64(&mut self, addr: &str, val: f64) -> Option<usize> {
        if let Some((index, p)) = self
            .params
            .iter_mut()
            .enumerate()
            .find(|(_, p)| p.addr() == addr)
        {
            p.update_f64(val);
            Some(index)
        } else {
            None
        }
    }

    pub fn update_param_s(&mut self, addr: &str, val: &str) -> Option<usize> {
        if let Some((index, p)) = self
            .params
            .iter_mut()
            .enumerate()
            .find(|(_, p)| p.addr() == addr)
        {
            p.update_s(val);
            Some(index)
        } else {
            None
        }
    }

    pub fn parse(index: usize, json: &serde_json::Value) -> Option<Self> {
        let contents = json.get("CONTENTS")?.as_object()?;
        let name = contents
            .get("name")?
            .as_object()?
            .get("VALUE")?
            .as_str()?
            .to_string();
        let params = Param::parse_all(index, contents.get("params")?).unwrap_or_default();
        let presets = parse_presets(&contents).unwrap_or_default();
        let data_refs = parse_datarefs(&contents).unwrap_or_default();

        Some(PatcherInst {
            index,
            name,
            params,
            presets,
            data_refs,
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
