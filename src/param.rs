/*{
  "FULL_PATH": "/rnbo/inst/0/params/damping",
  "TYPE": "f",
  "VALUE": 0.3499999940395355,
  "RANGE": [
    {
      "MIN": 0,
      "MAX": 1
    }
  ],
  "ACCESS": 3,
  "CLIPMODE": "both",
  "CONTENTS": {
    "normalized": {
      "FULL_PATH": "/rnbo/inst/0/params/damping/normalized",
      "TYPE": "f",
      "VALUE": 0.3499999940395355,
      "RANGE": [
        {
          "MIN": 0,
          "MAX": 1
        }
      ],
      "ACCESS": 3,
      "CLIPMODE": "both"
    }
  }
}
{
  "FULL_PATH": "/rnbo/inst/0/params/poly/delay/scale",
  "TYPE": "s",
  "VALUE": "x1",
  "RANGE": [
    {
      "VALS": [
        "1/4",
        "1/2",
        "3/4",
        "x1",
        "3/2",
        "x2",
        "x4"
      ]
    }
  ],
  "ACCESS": 3,
  "CLIPMODE": "both",
  "CONTENTS": {
    "normalized": {
      "FULL_PATH": "/rnbo/inst/0/params/poly/delay/scale/normalized",
      "TYPE": "f",
      "VALUE": 0.5,
      "RANGE": [
        {
          "MIN": 0,
          "MAX": 1
        }
      ],
      "ACCESS": 3,
      "CLIPMODE": "both"
    }
  }
}
*/

use {
    crate::util::parse_contents_meta,
    atomic_float::AtomicF64,
    palette::{Darken, Srgb},
    serde_json::Value,
    std::{
        cmp::PartialOrd,
        sync::atomic::Ordering,
        time::{Duration, Instant},
    },
};

const NORM_PENDING_DELAY: Duration = Duration::from_millis(50);
static GLOBAL_DELTA: AtomicF64 = AtomicF64::new(0.01);

fn get_color(v: f64) -> Srgb<u8> {
    let cap = 0.96;

    //TODO get from metdata?
    Srgb::new(1.0, 1.0, 1.0).darken(cap - v * cap).into_format()
}

fn get_delta(meta: &Value) -> Option<f64> {
    let meta = meta.as_object()?;
    if meta.contains_key("delta") {
        meta.get("delta")?
            .as_number()?
            .as_f64()
            .map(|v| v.clamp(0.0, 1.0))
    } else {
        None
    }
}

#[derive(Debug, Clone)]
pub enum ParamDetail {
    Float {
        val: f64,
        min: f64,
        max: f64,
        steps: Option<usize>,
    },
    Bool(bool),
    Enum(usize, Vec<String>),
}

#[derive(Debug, Clone)]
pub struct Param {
    index: usize,

    instance_index: usize,

    addr: String,
    addr_norm: String,

    name: String,
    detail: ParamDetail,

    display_name: Option<String>,
    display_order: Option<isize>,

    norm: f64,
    norm_pending: Option<(f64, Instant)>,

    norm_offset_step: Option<f64>,
    norm_offset_step_editable: bool,

    color: Srgb<u8>,

    meta: Value,
}

impl Param {
    pub fn global_delta() -> f64 {
        GLOBAL_DELTA.load(Ordering::Relaxed)
    }
    pub fn set_global_delta(delta: f64) {
        GLOBAL_DELTA.store(delta.clamp(0.0, 1.0), Ordering::Relaxed);
    }

    pub fn instance_index(&self) -> usize {
        self.instance_index
    }
    pub fn index(&self) -> usize {
        self.index
    }
    pub fn addr(&self) -> &str {
        self.addr.as_str()
    }

    pub fn addr_norm(&self) -> &str {
        self.addr_norm.as_str()
    }

    pub fn name(&self) -> &str {
        self.name.as_str()
    }

    pub fn display_name(&self) -> &str {
        self.display_name.as_ref().unwrap_or(&self.name).as_str()
    }

    pub fn display_order(&self) -> isize {
        self.display_order.unwrap_or(0)
    }

    pub fn color(&self) -> Srgb<u8> {
        self.color
    }

    pub fn norm(&mut self) -> f64 {
        if let Some((v, time)) = self.norm_pending
            && time + NORM_PENDING_DELAY < Instant::now()
        {
            self.norm = v;
            self.norm_pending = None;
        }
        self.norm
    }

    //get the pending value, useful for drawing
    pub fn norm_prefer_pending(&self) -> f64 {
        if let Some((v, _time)) = self.norm_pending {
            v
        } else {
            self.norm
        }
    }

    pub fn set_norm(&mut self, v: f64) {
        self.norm = v;
    }

    pub fn offset(&mut self, offset: isize) -> f64 {
        let offset_step = self
            .norm_offset_step
            .unwrap_or(GLOBAL_DELTA.load(Ordering::Relaxed));

        self.norm = (self.norm()
            + if offset > 0 {
                offset_step
            } else {
                -offset_step
            })
        .clamp(0.0, 1.0);
        self.norm
    }

    pub fn set_norm_pending(&mut self, v: f64) {
        self.color = get_color(v);
        self.norm_pending = Some((v, Instant::now()));
    }

    pub fn detail(&self) -> &ParamDetail {
        &self.detail
    }

    pub fn render_value(&self) -> String {
        match &self.detail {
            ParamDetail::Enum(index, vals) => vals[*index].clone(),
            ParamDetail::Bool(v) => if *v { "on" } else { "off" }.to_string(),
            ParamDetail::Float { val, .. } => {
                format!("{:.3}", val)
            }
        }
    }

    pub fn update_f64(&mut self, val: f64) {
        if let ParamDetail::Float {
            min, max, steps, ..
        } = self.detail
        {
            self.detail = ParamDetail::Float {
                val,
                min,
                max,
                steps,
            };
        }
    }

    pub fn update_s(&mut self, val: &str) {
        match &self.detail {
            ParamDetail::Enum(_, vals) => {
                let index = vals.iter().position(|v| v == val).unwrap_or(0);
                self.detail = ParamDetail::Enum(index, vals.clone()); //TODO get rid of this clone
            }
            ParamDetail::Bool(_) => {
                self.detail = ParamDetail::Bool(val == "1");
            }
            _ => (), //XXX error
        }
    }

    //returns true if visibility changes
    pub fn set_meta(&mut self, meta: &Value) -> bool {
        let hidden = self.hidden();
        self.meta = meta.clone();

        if self.norm_offset_step_editable {
            match self.detail {
                ParamDetail::Float { .. } => {
                    self.norm_offset_step = get_delta(meta);
                }
                _ => (),
            }
        }

        hidden != self.hidden()
    }

    pub fn set_delta(&mut self, v: Option<f64>) {
        if self.norm_offset_step_editable {
            match self.detail {
                ParamDetail::Float { .. } => {
                    self.norm_offset_step = v.map(|v| v.clamp(0.0, 1.0));
                }
                _ => (),
            }
        }
    }

    pub fn meta(&self) -> &Value {
        &self.meta
    }

    pub fn visible(&self) -> bool {
        !self.hidden()
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

    pub fn parse(instance_index: usize, json: &serde_json::Value) -> Option<Self> {
        if let serde_json::Value::Object(obj) = json {
            let range = obj.get("RANGE")?.as_array()?.first()?.as_object()?;
            let addr = obj.get("FULL_PATH")?.as_str()?.to_string();
            let name = addr.split("/params/").nth(1)?.to_string();
            let contents = obj.get("CONTENTS")?;
            let addr_norm = format!("{}/normalized", addr);

            let norm = contents
                .get("normalized")?
                .get("VALUE")?
                .as_number()?
                .as_f64()?;

            let meta = parse_contents_meta(contents).unwrap_or(Value::Null);

            let mut norm_offset_step = get_delta(&meta);
            let mut norm_offset_step_editable = true;

            let index = contents.get("index")?.get("VALUE")?.as_number()?.as_u64()? as usize;
            let color = get_color(norm);

            let display_name = if let Some(n) = contents.get("display_name") {
                let v = n.get("VALUE")?.as_str()?;
                if v.is_empty() {
                    None
                } else {
                    Some(v.to_string())
                }
            } else {
                None
            };

            let display_order = contents
                .get("display_order")
                .and_then(|n| n.get("VALUE")?.as_number()?.as_i64().map(|v| v as isize));

            match obj.get("TYPE")?.as_str()? {
                "s" => {
                    let vals: Option<Vec<String>> = range
                        .get("VALS")?
                        .as_array()?
                        .iter()
                        .map(|v| v.as_str().map(|v| v.to_string()))
                        .collect();
                    let vals = vals?;
                    let val = obj.get("VALUE")?.as_str()?;

                    let detail = if vals.len() == 2 && vals[0] == "0" && vals[1] == "1" {
                        norm_offset_step = Some(1.0);
                        norm_offset_step_editable = false;
                        ParamDetail::Bool(val == "1")
                    } else {
                        let index = vals.iter().position(|v| v == val).unwrap_or(0);
                        //normalized value is 0..1 inclusive so
                        //a 2 entry enum would be:
                        //1.0 / (2 * 2 - 1) =  0.333 -> 0, 0.33 | 0.66, 1.0
                        //a 3 entry enum would be:
                        //1.0 / (2 * 3 - 1) =  0.2 -> 0, 0.2 | 0.4, 0.6 | 0.8, 1.0
                        norm_offset_step = Some(1.0 / (2.0 * vals.len() as f64 - 1.0)); //half a step per change
                        norm_offset_step_editable = false;
                        ParamDetail::Enum(index, vals)
                    };
                    Some(Param {
                        index,
                        instance_index,
                        display_name,
                        display_order,
                        addr,
                        addr_norm,
                        name,
                        detail,
                        norm,
                        norm_pending: None,
                        norm_offset_step,
                        norm_offset_step_editable,
                        color,
                        meta,
                    })
                }
                "f" => {
                    let val = obj.get("VALUE")?.as_number()?.as_f64()?;
                    let (min, max) = (
                        range.get("MIN")?.as_number()?.as_f64()?,
                        range.get("MAX")?.as_number()?.as_f64()?,
                    );
                    let steps = contents
                        .get("steps")
                        .and_then(|s| s.get("VALUE")?.as_number()?.as_i64().map(|v| v as usize));
                    let detail = ParamDetail::Float {
                        val,
                        min,
                        max,
                        steps,
                    };
                    if let Some(steps) = steps
                        && steps > 1
                    {
                        norm_offset_step = Some(1.0 / (2.0 * steps as f64 - 1.0)); //0..1 inclusive
                        norm_offset_step_editable = false;
                    }
                    Some(Param {
                        index,
                        instance_index,
                        display_name,
                        display_order,
                        addr,
                        addr_norm,
                        name,
                        detail,
                        norm,
                        norm_pending: None,
                        norm_offset_step,
                        norm_offset_step_editable,
                        color,
                        meta,
                    })
                }
                _ => None,
            }
        } else {
            None
        }
    }

    //recursively parse for params
    fn get_all(
        instance_index: usize,
        json: &serde_json::Value,
        values: &mut Vec<Param>,
    ) -> Option<()> {
        for (_k, v) in json.get("CONTENTS")?.as_object()?.iter() {
            let c = v.get("CONTENTS")?.as_object()?;
            if c.contains_key("normalized") {
                values.push(Self::parse(instance_index, v)?);
            } else {
                Self::get_all(instance_index, v, values)?
            }
        }
        Some(())
    }

    pub fn parse_all(instance_index: usize, json: &serde_json::Value) -> Option<Vec<Param>> {
        //make sure we're at the right spot
        if json.get("FULL_PATH")?.as_str()?.ends_with("params") {
            let mut values: Vec<Param> = Vec::new();
            Self::get_all(instance_index, json, &mut values)?;
            values.sort_by(|a, b| a.index.partial_cmp(&b.index).unwrap());
            Some(values)
        } else {
            None
        }
    }
}

/*{
  "FULL_PATH": "/rnbo/inst/0/params",
  "DESCRIPTION": "Parameter get/set",
  "CONTENTS": {
    "gain": {
      "FULL_PATH": "/rnbo/inst/0/params/gain",
      "TYPE": "f",
      "VALUE": 0.5,
      "RANGE": [
        {
          "MIN": 0,
          "MAX": 1
        }
      ],
      "ACCESS": 3,
      "CLIPMODE": "both",
      "CONTENTS": {
        "normalized": {
          "FULL_PATH": "/rnbo/inst/0/params/gain/normalized",
          "TYPE": "f",
          "VALUE": 0.5,
          "RANGE": [
            {
              "MIN": 0,
              "MAX": 1
            }
          ],
          "ACCESS": 3,
          "CLIPMODE": "both"
        }
      }
    },
    "decay": {
      "FULL_PATH": "/rnbo/inst/0/params/decay",
      "TYPE": "f",
      "VALUE": 100.0000991821289,
      "RANGE": [
        {
          "MIN": 0.00009999999747378752,
          "MAX": 10000
        }
      ],
      "ACCESS": 3,
      "CLIPMODE": "both",
      "CONTENTS": {
        "normalized": {
          "FULL_PATH": "/rnbo/inst/0/params/decay/normalized",
          "TYPE": "f",
          "VALUE": 0.009999999776482582,
          "RANGE": [
            {
              "MIN": 0,
              "MAX": 1
            }
          ],
          "ACCESS": 3,
          "CLIPMODE": "both"
        }
      }
    },
    "damping": {
      "FULL_PATH": "/rnbo/inst/0/params/damping",
      "TYPE": "f",
      "VALUE": 0.3499999940395355,
      "RANGE": [
        {
          "MIN": 0,
          "MAX": 1
        }
      ],
      "ACCESS": 3,
      "CLIPMODE": "both",
      "CONTENTS": {
        "normalized": {
          "FULL_PATH": "/rnbo/inst/0/params/damping/normalized",
          "TYPE": "f",
          "VALUE": 0.3499999940395355,
          "RANGE": [
            {
              "MIN": 0,
              "MAX": 1
            }
          ],
          "ACCESS": 3,
          "CLIPMODE": "both"
        }
      }
    },
    "poly": {
      "FULL_PATH": "/rnbo/inst/0/params/poly",
      "CONTENTS": {
        "delay": {
          "FULL_PATH": "/rnbo/inst/0/params/poly/delay",
          "CONTENTS": {
            "input": {
              "FULL_PATH": "/rnbo/inst/0/params/poly/delay/input",
              "TYPE": "s",
              "VALUE": "1",
              "RANGE": [
                {
                  "VALS": [
                    "0",
                    "1"
                  ]
                }
              ],
              "ACCESS": 3,
              "CLIPMODE": "both",
              "CONTENTS": {
                "normalized": {
                  "FULL_PATH": "/rnbo/inst/0/params/poly/delay/input/normalized",
                  "TYPE": "f",
                  "VALUE": 1,
                  "RANGE": [
                    {
                      "MIN": 0,
                      "MAX": 1
                    }
                  ],
                  "ACCESS": 3,
                  "CLIPMODE": "both"
                }
              }
            },
            "time": {
              "FULL_PATH": "/rnbo/inst/0/params/poly/delay/time",
              "TYPE": "f",
              "VALUE": 50,
              "RANGE": [
                {
                  "MIN": 0,
                  "MAX": 100
                }
              ],
              "ACCESS": 3,
              "CLIPMODE": "both",
              "CONTENTS": {
                "normalized": {
                  "FULL_PATH": "/rnbo/inst/0/params/poly/delay/time/normalized",
                  "TYPE": "f",
                  "VALUE": 0.5,
                  "RANGE": [
                    {
                      "MIN": 0,
                      "MAX": 1
                    }
                  ],
                  "ACCESS": 3,
                  "CLIPMODE": "both"
                }
              }
            },
            "scale": {
              "FULL_PATH": "/rnbo/inst/0/params/poly/delay/scale",
              "TYPE": "s",
              "VALUE": "x1",
              "RANGE": [
                {
                  "VALS": [
                    "1/4",
                    "1/2",
                    "3/4",
                    "x1",
                    "3/2",
                    "x2",
                    "x4"
                  ]
                }
              ],
              "ACCESS": 3,
              "CLIPMODE": "both",
              "CONTENTS": {
                "normalized": {
                  "FULL_PATH": "/rnbo/inst/0/params/poly/delay/scale/normalized",
                  "TYPE": "f",
                  "VALUE": 0.5,
                  "RANGE": [
                    {
                      "MIN": 0,
                      "MAX": 1
                    }
                  ],
                  "ACCESS": 3,
                  "CLIPMODE": "both"
                }
              }
            },
            "color": {
              "FULL_PATH": "/rnbo/inst/0/params/poly/delay/color",
              "TYPE": "f",
              "VALUE": 50,
              "RANGE": [
                {
                  "MIN": 0,
                  "MAX": 100
                }
              ],
              "ACCESS": 3,
              "CLIPMODE": "both",
              "CONTENTS": {
                "normalized": {
                  "FULL_PATH": "/rnbo/inst/0/params/poly/delay/color/normalized",
                  "TYPE": "f",
                  "VALUE": 0.5,
                  "RANGE": [
                    {
                      "MIN": 0,
                      "MAX": 1
                    }
                  ],
                  "ACCESS": 3,
                  "CLIPMODE": "both"
                }
              }
            },
            "volume": {
              "FULL_PATH": "/rnbo/inst/0/params/poly/delay/volume",
              "TYPE": "f",
              "VALUE": -100,
              "RANGE": [
                {
                  "MIN": -100,
                  "MAX": 100
                }
              ],
              "ACCESS": 3,
              "CLIPMODE": "both",
              "CONTENTS": {
                "normalized": {
                  "FULL_PATH": "/rnbo/inst/0/params/poly/delay/volume/normalized",
                  "TYPE": "f",
                  "VALUE": 0,
                  "RANGE": [
                    {
                      "MIN": 0,
                      "MAX": 1
                    }
                  ],
                  "ACCESS": 3,
                  "CLIPMODE": "both"
                }
              }
            },
            "mix": {
              "FULL_PATH": "/rnbo/inst/0/params/poly/delay/mix",
              "TYPE": "f",
              "VALUE": 0,
              "RANGE": [
                {
                  "MIN": 0,
                  "MAX": 100
                }
              ],
              "ACCESS": 3,
              "CLIPMODE": "both",
              "CONTENTS": {
                "normalized": {
                  "FULL_PATH": "/rnbo/inst/0/params/poly/delay/mix/normalized",
                  "TYPE": "f",
                  "VALUE": 0,
                  "RANGE": [
                    {
                      "MIN": 0,
                      "MAX": 1
                    }
                  ],
                  "ACCESS": 3,
                  "CLIPMODE": "both"
                }
              }
            },
            "spread": {
              "FULL_PATH": "/rnbo/inst/0/params/poly/delay/spread",
              "TYPE": "f",
              "VALUE": -52,
              "RANGE": [
                {
                  "MIN": -100,
                  "MAX": 100
                }
              ],
              "ACCESS": 3,
              "CLIPMODE": "both",
              "CONTENTS": {
                "normalized": {
                  "FULL_PATH": "/rnbo/inst/0/params/poly/delay/spread/normalized",
                  "TYPE": "f",
                  "VALUE": 0.23999999463558197,
                  "RANGE": [
                    {
                      "MIN": 0,
                      "MAX": 1
                    }
                  ],
                  "ACCESS": 3,
                  "CLIPMODE": "both"
                }
              }
            },
            "filter": {
              "FULL_PATH": "/rnbo/inst/0/params/poly/delay/filter",
              "TYPE": "s",
              "VALUE": "None",
              "RANGE": [
                {
                  "VALS": [
                    "None",
                    "LP",
                    "HP",
                    "BP",
                    "Notch"
                  ]
                }
              ],
              "ACCESS": 3,
              "CLIPMODE": "both",
              "CONTENTS": {
                "normalized": {
                  "FULL_PATH": "/rnbo/inst/0/params/poly/delay/filter/normalized",
                  "TYPE": "f",
                  "VALUE": 0.029999999329447746,
                  "RANGE": [
                    {
                      "MIN": 0,
                      "MAX": 1
                    }
                  ],
                  "ACCESS": 3,
                  "CLIPMODE": "both"
                }
              }
            },
            "regen": {
              "FULL_PATH": "/rnbo/inst/0/params/poly/delay/regen",
              "TYPE": "f",
              "VALUE": 50,
              "RANGE": [
                {
                  "MIN": 0,
                  "MAX": 100
                }
              ],
              "ACCESS": 3,
              "CLIPMODE": "both",
              "CONTENTS": {
                "normalized": {
                  "FULL_PATH": "/rnbo/inst/0/params/poly/delay/regen/normalized",
                  "TYPE": "f",
                  "VALUE": 0.5,
                  "RANGE": [
                    {
                      "MIN": 0,
                      "MAX": 1
                    }
                  ],
                  "ACCESS": 3,
                  "CLIPMODE": "both"
                }
              }
            },
            "fb": {
              "FULL_PATH": "/rnbo/inst/0/params/poly/delay/fb",
              "TYPE": "s",
              "VALUE": "1",
              "RANGE": [
                {
                  "VALS": [
                    "0",
                    "1"
                  ]
                }
              ],
              "ACCESS": 3,
              "CLIPMODE": "both",
              "CONTENTS": {
                "normalized": {
                  "FULL_PATH": "/rnbo/inst/0/params/poly/delay/fb/normalized",
                  "TYPE": "f",
                  "VALUE": 1,
                  "RANGE": [
                    {
                      "MIN": 0,
                      "MAX": 1
                    }
                  ],
                  "ACCESS": 3,
                  "CLIPMODE": "both"
                }
              }
            }
          }
        },
        "string": {
          "FULL_PATH": "/rnbo/inst/0/params/poly/string",
          "CONTENTS": {
            "osc.analog_1_": {
              "FULL_PATH": "/rnbo/inst/0/params/poly/string/osc.analog_1_",
              "CONTENTS": {
                "damping": {
                  "FULL_PATH": "/rnbo/inst/0/params/poly/string/osc.analog_1_/damping",
                  "TYPE": "f",
                  "VALUE": 0.800000011920929,
                  "RANGE": [
                    {
                      "MIN": 0,
                      "MAX": 1
                    }
                  ],
                  "ACCESS": 3,
                  "CLIPMODE": "both",
                  "CONTENTS": {
                    "normalized": {
                      "FULL_PATH": "/rnbo/inst/0/params/poly/string/osc.analog_1_/damping/normalized",
                      "TYPE": "f",
                      "VALUE": 0.800000011920929,
                      "RANGE": [
                        {
                          "MIN": 0,
                          "MAX": 1
                        }
                      ],
                      "ACCESS": 3,
                      "CLIPMODE": "both"
                    }
                  }
                }
              }
            },
            "osc.analog_1_.1": {
              "FULL_PATH": "/rnbo/inst/0/params/poly/string/osc.analog_1_.1",
              "CONTENTS": {
                "decay": {
                  "FULL_PATH": "/rnbo/inst/0/params/poly/string/osc.analog_1_.1/decay",
                  "TYPE": "f",
                  "VALUE": 0.800000011920929,
                  "RANGE": [
                    {
                      "MIN": 0.00009999999747378752,
                      "MAX": 10000
                    }
                  ],
                  "ACCESS": 3,
                  "CLIPMODE": "both",
                  "CONTENTS": {
                    "normalized": {
                      "FULL_PATH": "/rnbo/inst/0/params/poly/string/osc.analog_1_.1/decay/normalized",
                      "TYPE": "f",
                      "VALUE": 0.00007999000081326813,
                      "RANGE": [
                        {
                          "MIN": 0,
                          "MAX": 1
                        }
                      ],
                      "ACCESS": 3,
                      "CLIPMODE": "both"
                    }
                  }
                }
              }
            }
          }
        }
      }
    }
  }
}
*/
