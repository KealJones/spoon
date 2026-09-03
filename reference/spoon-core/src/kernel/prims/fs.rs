//! Filesystem primitives (fs.*). Effect::Read or Write. All check the sandbox.

use std::io::Read as IoRead;

use crate::types::*;

use super::super::{Ctx, EvalError, Kernel};

pub fn register(k: &mut Kernel) {
    use Effect::{Pure, Read, Write};

    macro_rules! p {
        ($id:expr, $verbs:expr, $inputs:expr, $out:expr, $eff:expr, $desc:expr, $f:expr) => {
            k.register(Action::primitive($id, $verbs, $inputs, $out, $eff, $desc), $f)
        };
    }

    let pa = || Type::Path;
    let t = || Type::Text;
    let b = || Type::Bool;
    let lp = || Type::list(Type::Path);

    p!("fs.read",      &["read"],     vec![Input::required("path", pa())], t(),  Read,  "read text content of a file",                fs_read);
    p!("fs.write",     &["write"],    vec![Input::required("path", pa()), Input::required("content", t())], b(), Write, "write text to a file (overwrite)", fs_write);
    p!("fs.append",    &["append"],   vec![Input::required("path", pa()), Input::required("content", t())], b(), Write, "append text to a file",            fs_append);
    p!("fs.list_dir",  &["list-dir"], vec![Input::required("path", pa())], lp(), Read,  "list files and directories in a directory", fs_list_dir);
    p!("fs.exists",    &["exists"],   vec![Input::required("path", pa())], b(),  Read,  "true if path exists",                       fs_exists);
    p!("fs.is_dir",    &["is-dir"],   vec![Input::required("path", pa())], b(),  Read,  "true if path is a directory",               fs_is_dir);
    p!("fs.basename",  &["basename"], vec![Input::required("path", pa())], t(),  Pure,  "file name component of a path",             fs_basename);
    p!("fs.dirname",   &["dirname"],  vec![Input::required("path", pa())], pa(), Pure,  "directory component of a path",             fs_dirname);
    p!("fs.extension", &["extension"],vec![Input::required("path", pa())], t(),  Pure,  "file extension (without dot)",              fs_extension);
    p!("fs.join",      &["join"],     vec![Input::required("path", pa()), Input::required("part", t())], pa(), Pure, "join a path with a component", fs_join);
    p!("fs.size",      &["size"],     vec![Input::required("path", pa())], Type::Int, Read, "file size in bytes",                     fs_size);
    p!("fs.mkdir",     &["mkdir"],    vec![Input::required("path", pa())], b(),  Write, "create directory (and parents)",            fs_mkdir);
    p!("fs.delete",    &["delete"],   vec![Input::required("path", pa())], b(),  Write, "delete a file or empty directory",          fs_delete);
}

// ---- helpers -----------------------------------------------------------

fn path_arg(ctx: &Ctx<'_>, id: &ActionId, args: &[Value], i: usize) -> Result<String, EvalError> {
    let p = match args.get(i) {
        Some(Value::Path(s)) | Some(Value::Text(s)) => s.clone(),
        Some(Value::Name(s)) => s.clone(),
        other => return Err(EvalError::ty("path", other.unwrap_or(&Value::Null), &id.0)),
    };
    if !ctx.kernel.sandbox.path_allowed(&p) {
        return Err(EvalError::runtime(id, format!("path not allowed: {p}")));
    }
    Ok(p)
}

// ---- primitives --------------------------------------------------------

fn fs_read(ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let id = ActionId("fs.read".into());
    let path = path_arg(ctx, &id, args, 0)?;
    let mut file = std::fs::File::open(&path)
        .map_err(|e| EvalError::runtime(&id, format!("{}: {}", path, e)))?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)
        .map_err(|e| EvalError::runtime(&id, e.to_string()))?;
    if buf.len() > ctx.kernel.sandbox.max_read_bytes {
        return Err(EvalError::runtime(&id, format!("file too large ({} bytes)", buf.len())));
    }
    Ok(Value::Text(String::from_utf8_lossy(&buf).into_owned()))
}

fn fs_write(ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let id = ActionId("fs.write".into());
    let path = path_arg(ctx, &id, args, 0)?;
    let content = args.get(1).and_then(Value::as_str)
        .ok_or_else(|| EvalError::ty("text", args.get(1).unwrap_or(&Value::Null), "fs.write"))?;
    std::fs::write(&path, content)
        .map_err(|e| EvalError::runtime(&id, e.to_string()))?;
    Ok(Value::Bool(true))
}

fn fs_append(ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    use std::io::Write;
    let id = ActionId("fs.append".into());
    let path = path_arg(ctx, &id, args, 0)?;
    let content = args.get(1).and_then(Value::as_str)
        .ok_or_else(|| EvalError::ty("text", args.get(1).unwrap_or(&Value::Null), "fs.append"))?;
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&path)
        .map_err(|e| EvalError::runtime(&id, e.to_string()))?;
    file.write_all(content.as_bytes())
        .map_err(|e| EvalError::runtime(&id, e.to_string()))?;
    Ok(Value::Bool(true))
}

fn fs_list_dir(ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let id = ActionId("fs.list_dir".into());
    let path = path_arg(ctx, &id, args, 0)?;
    let entries = std::fs::read_dir(&path)
        .map_err(|e| EvalError::runtime(&id, e.to_string()))?;
    let paths: Result<Vec<Value>, _> = entries
        .map(|e| e.map(|e| Value::Path(e.path().to_string_lossy().into_owned())))
        .collect::<std::io::Result<_>>()
        .map_err(|e| EvalError::runtime(&id, e.to_string()));
    Ok(Value::List(paths?))
}

fn fs_exists(ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let id = ActionId("fs.exists".into());
    let path = path_arg(ctx, &id, args, 0)?;
    Ok(Value::Bool(std::path::Path::new(&path).exists()))
}

fn fs_is_dir(ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let id = ActionId("fs.is_dir".into());
    let path = path_arg(ctx, &id, args, 0)?;
    Ok(Value::Bool(std::path::Path::new(&path).is_dir()))
}

fn fs_basename(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let p = args.first().and_then(Value::as_str)
        .ok_or_else(|| EvalError::ty("path", args.first().unwrap_or(&Value::Null), "fs.basename"))?;
    let name = std::path::Path::new(p)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok(Value::Text(name))
}

fn fs_dirname(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let p = args.first().and_then(Value::as_str)
        .ok_or_else(|| EvalError::ty("path", args.first().unwrap_or(&Value::Null), "fs.dirname"))?;
    let dir = std::path::Path::new(p)
        .parent()
        .map(|d| d.to_string_lossy().into_owned())
        .unwrap_or_else(|| ".".into());
    Ok(Value::Path(dir))
}

fn fs_extension(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let p = args.first().and_then(Value::as_str)
        .ok_or_else(|| EvalError::ty("path", args.first().unwrap_or(&Value::Null), "fs.extension"))?;
    let ext = std::path::Path::new(p)
        .extension()
        .map(|e| e.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok(Value::Text(ext))
}

fn fs_join(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let base = args.first().and_then(Value::as_str)
        .ok_or_else(|| EvalError::ty("path", args.first().unwrap_or(&Value::Null), "fs.join"))?;
    let part = args.get(1).and_then(Value::as_str)
        .ok_or_else(|| EvalError::ty("text", args.get(1).unwrap_or(&Value::Null), "fs.join"))?;
    let joined = std::path::Path::new(base).join(part).to_string_lossy().into_owned();
    Ok(Value::Path(joined))
}

fn fs_size(ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let id = ActionId("fs.size".into());
    let path = path_arg(ctx, &id, args, 0)?;
    let meta = std::fs::metadata(&path)
        .map_err(|e| EvalError::runtime(&id, e.to_string()))?;
    Ok(Value::Int(meta.len() as i64))
}

fn fs_mkdir(ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let id = ActionId("fs.mkdir".into());
    let path = path_arg(ctx, &id, args, 0)?;
    std::fs::create_dir_all(&path)
        .map_err(|e| EvalError::runtime(&id, e.to_string()))?;
    Ok(Value::Bool(true))
}

fn fs_delete(ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let id = ActionId("fs.delete".into());
    let path = path_arg(ctx, &id, args, 0)?;
    let p = std::path::Path::new(&path);
    if p.is_dir() {
        std::fs::remove_dir(&path).map_err(|e| EvalError::runtime(&id, e.to_string()))?;
    } else {
        std::fs::remove_file(&path).map_err(|e| EvalError::runtime(&id, e.to_string()))?;
    }
    Ok(Value::Bool(true))
}
