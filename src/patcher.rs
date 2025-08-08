use {
    crate::{param::Param, util::parse_meta},
    serde_json::Value,
    std::collections::{BTreeMap, HashMap},
};

pub struct PatcherInst {
    index: usize,
    name: String,
    alias: Option<String>,
    params: Vec<Param>,
    presets: Vec<String>,
    datarefs: BTreeMap<String, Dataref>,
    visible_datarefs: Vec<String>, //keys from datarefs
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
) -> Option<BTreeMap<String, Dataref>> {
    let mut datarefs = BTreeMap::new();

    for (name, body) in contents
        .get("data_refs")?
        .as_object()?
        .get("CONTENTS")?
        .as_object()?
        .iter()
    {
        let mapping = body.get("VALUE")?.as_str()?;
        let mapping = if !mapping.is_empty() {
            Some(mapping.to_string())
        } else {
            None
        };

        let meta = parse_meta(body).unwrap_or(Value::Null);

        let mapping = Dataref { mapping, meta };

        datarefs.insert(name.clone(), mapping);
    }

    Some(datarefs)
}

#[derive(Clone)]
pub struct Dataref {
    mapping: Option<String>,
    meta: Value,
}

impl Dataref {
    pub fn mapping(&self) -> &Option<String> {
        &self.mapping
    }
    pub fn mapping_mut(&mut self) -> &mut Option<String> {
        &mut self.mapping
    }

    //returns true if visibility changes
    pub fn set_meta(&mut self, meta: &Value) -> bool {
        let hidden = self.hidden();
        self.meta = meta.clone();
        hidden != self.hidden()
    }

    pub fn meta(&self) -> &Value {
        &self.meta
    }

    pub fn hidden(&self) -> bool {
        if let Some(meta) = self.meta.as_object()
            && meta.contains_key("hidden")
            && let Some(hidden) = meta.get("hidden")
            && let Some(v) = hidden.as_bool()
        {
            return v;
        }
        false
    }
}

impl PatcherInst {
    pub fn index(&self) -> usize {
        self.index
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn alias(&self) -> Option<&String> {
        self.alias.as_ref()
    }
    pub fn alias_or_index_name(&self) -> String {
        self.alias
            .clone()
            .unwrap_or_else(|| format!("{}: {}", self.index, self.name))
    }
    pub fn set_alias(&mut self, alias: Option<String>) {
        self.alias = alias;
    }

    pub fn params(&self) -> &Vec<Param> {
        &self.params
    }

    pub fn params_mut(&mut self) -> &mut Vec<Param> {
        &mut self.params
    }

    pub fn visible_datarefs(&self) -> &Vec<String> {
        &self.visible_datarefs
    }

    pub fn visibile_datarefs_mut(&mut self) -> &mut Vec<String> {
        &mut self.visible_datarefs
    }

    pub fn dataref_mappings(&self) -> &BTreeMap<String, Dataref> {
        &self.datarefs
    }

    pub fn dataref_mappings_mut(&mut self) -> &mut BTreeMap<String, Dataref> {
        &mut self.datarefs
    }

    pub fn update_visible_datarefs(&mut self) {
        self.visible_datarefs = self
            .datarefs
            .iter()
            .filter(|(_, dref)| !dref.hidden())
            .map(|(key, _)| key.clone())
            .collect();
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
        let presets = parse_presets(contents).unwrap_or_default();
        let datarefs = parse_datarefs(contents).unwrap_or_default();

        let get_alias = || -> Option<String> {
            let alias = contents
                .get("config")?
                .as_object()?
                .get("CONTENTS")?
                .as_object()?
                .get("name_alias")?
                .as_object()?
                .get("VALUE")?
                .as_str()?;
            if !alias.is_empty() {
                Some(alias.to_string())
            } else {
                None
            }
        };

        let mut inst = PatcherInst {
            index,
            name,
            alias: get_alias(),
            params,
            presets,
            datarefs,
            visible_datarefs: Vec::new(),
        };
        inst.update_visible_datarefs();
        Some(inst)
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
