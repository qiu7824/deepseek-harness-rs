# schemastery Rust 移植关键决策与偏差记录

> 供后续轮次(loader/include、settings 持久化)参考。基线:`vendor/schemastery`
> v3.18.1,单文件 `src/index.ts`(902 行)。Rust 位于
> `crates/vendor/schemastery`(crate `dsh-schemastery`)。

## 1. 结构映射

| TS | Rust |
|---|---|
| callable `Schema` + prototype 方法 | `Schema { inner: Arc<SchemaInner> }`,构建器返回共享节点的派生 Schema(新 uid + meta 覆写) |
| `type` + 同层 payload 字段(inner/list/dict/...) | `Node` 枚举(每变体携带 payload) |
| `Schema(this)` 浅拷贝 | `Schema::derive()` |
| `schema(data)` 调用校验 | `Schema::validate(&schema, data)` |
| `Schema.resolve(data, schema, options, strict)` → `[value, adapted]` | `Schema::resolve(&mut Data, &Schema, &Options, bool) -> Result<Data>`;adapted 写回在节点解析器内部完成(dict 键改名、transform 非 preserve) |
| `~standard` | `StandardSchemaV1` trait + `standard_validate` |
| `ValidationError extends TypeError` | `ValidationError { message, path }`,Display 含 `$` 前缀路径 |
| JS `any` | `Data` 枚举(见下) |

## 2. Data 模型

`Data` = `Null / Undefined / Bool / Number / String / Array(Vec) /
Object(IndexMap) / Date(chrono DateTime<Utc>) / RegExp { source, flags } /
Binary(Vec<u8>) / Instance { name, Arc<dyn Any> }`。

- `Null`+`Undefined` 均为 nullish(对应 TS `null`/`undefined`)。
- `Schema.is(name)` 按名字匹配 `Instance`;`Date`/`RegExp`/`Binary` 对
  `"Date"`/`"RegExp"`/`"ArrayBuffer"|"SharedArrayBuffer"` 自动匹配。
- `to_json()` 只在 JSON 兼容时返回 `Some(serde_json::Value)`。
- `deep_equal`/`PartialEq` 按 TS `deepEqual` 语义实现(Instance 用指针身份)。

## 3. 已实现 / 未实现

已实现:全部 20 节点类型(any/never/const/string/number/boolean/bitset/
function/is/array/dict/tuple/object/union/intersect/transform/lazy)、
`natural`/`percent`/`date`/`regExp`/`arrayBuffer`、meta 链
(required/hidden/loose/disabled/collapse/role/link/default/comment/
description/max/min/step/pattern/deprecated/experimental/extra)、`set`/`push`、
`simplify`、`i18n`、`type_string`(TS `toString` 格式化)、`from`、
Standard Schema V1。

未实现(记录在 lib.rs 偏差节):
- `toJSON`/`fromJSON` schema 序列化(M2 settings 持久化时补);
- TS `is(Constructor)` 的类身份(仅名字匹配);
- `new Function(...)` 字符串 callback 反序列化。

## 4. 语义细节(易错点,勿改回)

- **`deepEqual` 第三参是 strict 标志**:`simplify` 首行
  `deepEqual(value, meta.default, type === 'dict')` 是「dict 用严格比较」,
  不是「dict 跳过比较」。
- **无依赖 regex flags**:`build_regex` 在 flags 为空时不得输出 `(?)` 前缀。
- **`isMultipleOf`**:step 形如 `^\d+\.\d+$` 走 `decimalShift` 字符串移位路径,
  否则直接浮点取模;`decimalShift` 用 Rust f64 Display 近似 JS toString。
- **string length** 用 `chars().count()`(TS 是 UTF-16 code unit 数,BMP 内一致)。
- **bitset**:输出 value;value ≠ default 时 adapted = key 数组(输入被改写)。
- **object**:属性 `!nullish || key in data` 才入结果;非 strict 合并未知键。
- **tuple**:非 strict 保留超出 list 长度的剩余元素。
- **intersect**:成员用 `strict=true` 解析;typeof(判别式)不同即报错;
  Array↔Array 的 JS 索引合并未移植(见代码注释)。
- **lazy**:首次解析时构建并 `{ ...lazy.meta, ...inner.meta }` 覆写;派生的
  lazy 节点共享 builder、各自空 cache。
- **transform**:`preserve=true` 只回调 result;`false` 时 adapted 写回调用方
  持有的输入(TS 由 `property()` 写回,Rust 在解析器内写回,顶层可见性相同)。
- **autofix**:`property()` 捕获子错误时 `delete data[key]` 并返回该属性的
  default。

## 5. 测试

21 项,覆盖:标量、嵌套路径前缀(`$.a.b[1]`)、required/default、number step
(整数与小数两条路径)、pattern/长度、bitset 归一化、union 顺序与报错文案、
intersect 合并、object 非 strict 合并、transform 双模式、dict sKey 改名、
lazy 递归树、autofix、loose、`~standard` 结果形状、`from`、simplify、
date/regExp/arrayBuffer、`type_string` 格式、tuple 余项保留。
