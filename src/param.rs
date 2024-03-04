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

#[derive(Debug)]
pub enum ParamDetail {
    Float { val: f64, min: f64, max: f64 },
    Bool(bool),
    Enum(usize, Vec<String>),
}

#[derive(Debug)]
pub struct Param {
    index: usize,
    addr: String,
    name: String,
    detail: ParamDetail,
    norm: f64,
}

impl Param {
    pub fn index(&self) -> usize {
        self.index
    }
    pub fn addr(&self) -> &str {
        self.addr.as_str()
    }

    pub fn name(&self) -> &str {
        self.name.as_str()
    }

    pub fn norm(&self) -> f64 {
        self.norm
    }

    pub fn set_norm(&mut self, v: f64) {
        self.norm = v;
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
        match self.detail {
            ParamDetail::Float { min, max, .. } => {
                self.detail = ParamDetail::Float { val, min, max };
            }
            _ => (), //XXX error
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

    pub fn update_norm(&mut self, v: f64) {
        self.norm = v;
    }

    pub fn parse(json: &serde_json::Value) -> Option<Self> {
        if let serde_json::Value::Object(obj) = json {
            let range = obj.get("RANGE")?.as_array()?.get(0)?.as_object()?;
            let addr = obj.get("FULL_PATH")?.as_str()?.to_string();
            let name = addr.split("/params/").nth(1)?.to_string();
            let contents = obj.get("CONTENTS")?;

            let norm = contents
                .get("normalized")?
                .get("VALUE")?
                .as_number()?
                .as_f64()?;

            let index = contents.get("index")?.get("VALUE")?.as_number()?.as_u64()? as usize;

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
                        ParamDetail::Bool(val == "1")
                    } else {
                        let index = vals.iter().position(|v| v == val).unwrap_or(0);
                        ParamDetail::Enum(index, vals)
                    };
                    Some(Param {
                        index,
                        addr,
                        name,
                        detail,
                        norm,
                    })
                }
                "f" => {
                    let val = obj.get("VALUE")?.as_number()?.as_f64()?;
                    let (min, max) = (
                        range.get("MIN")?.as_number()?.as_f64()?,
                        range.get("MAX")?.as_number()?.as_f64()?,
                    );
                    let detail = ParamDetail::Float { val, min, max };
                    Some(Param {
                        index,
                        addr,
                        name,
                        detail,
                        norm,
                    })
                }
                _ => None,
            }
        } else {
            None
        }
    }

    //recursively parse for params
    fn get_all(json: &serde_json::Value, values: &mut Vec<Param>) -> Option<()> {
        for (k, v) in json.get("CONTENTS")?.as_object()?.iter() {
            let c = v.get("CONTENTS")?.as_object()?;
            if c.contains_key("normalized") {
                values.push(Self::parse(v)?);
            } else {
                Self::get_all(v, values)?
            }
        }
        Some(())
    }

    pub fn parse_all(json: &serde_json::Value) -> Option<Vec<Param>> {
        //make sure we're at the right spot
        if json.get("FULL_PATH")?.as_str()?.ends_with("params") {
            let mut values: Vec<Param> = Vec::new();
            Self::get_all(json, &mut values)?;
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
