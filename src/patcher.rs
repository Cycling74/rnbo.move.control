use {crate::param::Param, std::collections::HashMap};

#[derive(Debug)]
pub struct PatcherInst {
    index: usize,
    name: String,
    params: Vec<Param>,
    presets: Vec<String>,
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
        let params = Param::parse_all(contents.get("params")?)?;
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
        Some(PatcherInst {
            index,
            name,
            params,
            presets,
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
