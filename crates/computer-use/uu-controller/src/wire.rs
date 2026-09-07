use serde_json::{Value, json};
pub enum Field<'a> {
    Number(u32, u64),
    Bytes(u32, &'a [u8]),
}
fn varint(data: &[u8], at: &mut usize) -> Option<u64> {
    let mut v = 0;
    for shift in (0..70).step_by(7) {
        let b = *data.get(*at)?;
        *at += 1;
        if shift == 63 && b > 1 {
            return None;
        }
        v |= ((b & 127) as u64) << shift;
        if b & 128 == 0 {
            return Some(v);
        }
    }
    None
}
pub fn fields(data: &[u8]) -> Option<Vec<Field<'_>>> {
    let mut at = 0;
    let mut out = Vec::new();
    while at < data.len() {
        if out.len() > 1024 {
            return None;
        }
        let tag = varint(data, &mut at)?;
        let n = (tag >> 3) as u32;
        if n == 0 {
            return None;
        }
        match tag & 7 {
            0 => out.push(Field::Number(n, varint(data, &mut at)?)),
            2 => {
                let len = usize::try_from(varint(data, &mut at)?).ok()?;
                let end = at.checked_add(len)?;
                out.push(Field::Bytes(n, data.get(at..end)?));
                at = end
            }
            1 => {
                at = at.checked_add(8)?;
                if at > data.len() {
                    return None;
                }
            }
            5 => {
                at = at.checked_add(4)?;
                if at > data.len() {
                    return None;
                }
            }
            _ => return None,
        }
    }
    Some(out)
}
fn nums(data: &[u8], depth: usize) -> Value {
    if depth > 3 {
        return Value::Null;
    }
    json!(
        fields(data)
            .unwrap_or_default()
            .iter()
            .filter_map(|f| match f {
                Field::Number(n, v) => Some(json!({"field":n,"value":v})),
                Field::Bytes(n, v) => Some(json!({"field":n,"nested":nums(v,depth+1)})),
            })
            .collect::<Vec<_>>()
    )
}
pub fn metadata(kind: &str, data: &[u8]) -> Value {
    let Some(items) = fields(data) else {
        return json!({"kind":kind,"length":data.len(),"protobuf":false});
    };
    let mut result = json!({"kind":kind,"length":data.len(),"fields":items.iter().map(|x|match x{Field::Number(n,_)|Field::Bytes(n,_)=>*n}).collect::<Vec<_>>()});
    for field in &items {
        if let Field::Bytes(n, payload) = field {
            if [8, 14, 15, 18, 22].contains(n) {
                result[format!("status{n}")] = nums(payload, 0);
            }
        }
    }
    for field in items {
        if let Field::Bytes(7, sources) = field {
            if let Some(items) = fields(sources) {
                let mut screens = Vec::new();
                let mut current = 0;
                for item in items {
                    match item {
                        Field::Number(2, id) => current = id,
                        Field::Bytes(1, screen) => {
                            let mut value = json!({"id":0,"track":0});
                            if let Some(entries) = fields(screen) {
                                for entry in entries {
                                    match entry {
                                        Field::Number(1, id) => value["id"] = json!(id),
                                        Field::Number(12, id) => value["track"] = json!(id),
                                        Field::Number(7, b) => value["primary"] = json!(b != 0),
                                        Field::Bytes(4, rect) => {
                                            if let Some(nums) = fields(rect) {
                                                for f in nums {
                                                    if let Field::Number(n, v) = f {
                                                        if let Some(key) = [
                                                            "left",
                                                            "top",
                                                            "width",
                                                            "height",
                                                            "pixelWidth",
                                                            "pixelHeight",
                                                        ]
                                                        .get(n.wrapping_sub(1) as usize)
                                                        {
                                                            value[*key] = json!(v as i64)
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            screens.push(value)
                        }
                        _ => {}
                    }
                }
                result["screens"] = json!(screens);
                result["current"] = json!(current)
            }
        }
    }
    result
}
fn push_var(out: &mut Vec<u8>, mut v: u64) {
    while v >= 128 {
        out.push((v as u8 & 127) | 128);
        v >>= 7
    }
    out.push(v as u8)
}
fn bytes(n: u32, value: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    push_var(&mut out, ((n as u64) << 3) | 2);
    push_var(&mut out, value.len() as u64);
    out.extend_from_slice(value);
    out
}
pub fn request_track(track: u32) -> Vec<u8> {
    let mut index = Vec::new();
    push_var(&mut index, track as u64);
    let tracks = bytes(1, &index);
    let mut request = bytes(1, &[8, 1]);
    request.extend(bytes(15, &tracks));
    let mut message = vec![8, 1];
    message.extend(bytes(21, &request));
    message
}
pub fn request_capture() -> Vec<u8> {
    let mut settings = Vec::new();
    for (n, v) in [
        (1, 1),
        (2, 3),
        (3, 1),
        (4, 0),
        (7, 1),
        (10, 3),
        (12, 3),
        (13, 1),
        (14, 1920),
        (15, 1080),
        (18, 30),
    ] {
        push_var(&mut settings, n << 3);
        push_var(&mut settings, v)
    }
    let mut rpc = bytes(1, &[8, 2]);
    rpc.extend(bytes(2, &settings));
    let mut message = vec![8, 2];
    message.extend(bytes(21, &rpc));
    message
}
pub fn start_desktop() -> Vec<u8> {
    let mut message = vec![8, 3];
    message.extend(bytes(3, &[8, 4]));
    message
}
pub fn echo_reply(data: &[u8]) -> Option<Vec<u8>> {
    for field in fields(data)? {
        if let Field::Bytes(3, payload) = field {
            let mut action = 0;
            let mut args = None;
            for f in fields(payload)? {
                match f {
                    Field::Number(1, n) => action = n,
                    Field::Bytes(2, v) if v.len() < 4096 => args = Some(v),
                    _ => {}
                }
            }
            if action == 0 {
                let mut body = vec![8, 1];
                if let Some(args) = args {
                    body.extend(bytes(2, args));
                }
                return Some(bytes(3, &body));
            }
        }
    }
    None
}
