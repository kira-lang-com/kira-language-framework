//! Module scope (sections 9, 10, 13 and 14): constants, overrides, globals,
//! functions, entry points and the shader declaration.

use super::{Args, NONE, PendingFunction, ShaderState};
use klf_shader::parse::fragment;
use klf_shader::{
    FunctionArgument, FunctionResult, GroupClass, GroupDecl, ResourceBinding, ShaderKind, Stage,
    WorkgroupExtent,
};

fn word(value: impl TryInto<u64>) -> Result<u64, String> {
    value
        .try_into()
        .map_err(|_| "a handle out of range".to_owned())
}

impl ShaderState {
    /// `klf.shader.constant(module, name, type, init)`; `init` is a global
    /// expression.
    pub(super) fn constant(&mut self, args: &Args) -> Result<u64, String> {
        let (ty, init) = (args.handle(1)?, args.handle(2)?);
        let build = self.build(args.word(0)?)?;
        let span = build.span;
        let handle = build
            .builder
            .constant(args.text(0)?, ty, init, span)
            .map_err(|error| error.0)?;
        word(handle.raw())
    }

    /// `klf.shader.override(module, name, id, type, init)`; `id` and
    /// `init` may be `-1`.
    pub(super) fn override_(&mut self, args: &Args) -> Result<u64, String> {
        let id = match args.int(1)? {
            -1 => None,
            id => Some(u16::try_from(id).map_err(|_| format!("override id {id}"))?),
        };
        let (ty, init) = (args.handle(2)?, args.optional(3)?);
        let build = self.build(args.word(0)?)?;
        let span = build.span;
        let handle = build
            .builder
            .override_(args.text(0)?, id, ty, init, span)
            .map_err(|error| error.0)?;
        word(handle.raw())
    }

    /// `klf.shader.global(module, name, space, type, group, binding)`; a
    /// `group` of `-1` declares no resource binding.
    pub(super) fn global(&mut self, args: &Args) -> Result<u64, String> {
        let space = fragment::space(args.text(1)?)?;
        let ty = args.handle(1)?;
        let binding = match args.word(2)? {
            NONE => None,
            _ => Some(ResourceBinding {
                group: args.small(2)?,
                binding: args.small(3)?,
            }),
        };
        let build = self.build(args.word(0)?)?;
        let span = build.span;
        let handle = build
            .builder
            .global(args.text(0)?, space, ty, binding, span)
            .map_err(|error| error.0)?;
        word(handle.raw())
    }

    /// `klf.shader.slot(module, global, slot)`: an authored slot (section
    /// 9.4), such as `universal 3`.
    pub(super) fn slot(&mut self, args: &Args) -> Result<u64, String> {
        let slot = fragment::slot(args.text(0)?)?;
        let global = args.handle(1)?;
        self.build(args.word(0)?)?
            .builder
            .authored_slot(global, slot)
            .map_err(|error| error.0)?;
        Ok(0)
    }

    /// `klf.shader.param(module, name, type, binding)`: one parameter of
    /// the next function or entry point.
    pub(super) fn param(&mut self, args: &Args) -> Result<u64, String> {
        let argument = FunctionArgument {
            name: Some(args.text(0)?.to_owned()),
            ty: args.handle(1)?,
            binding: fragment::binding(args.text(1)?)?,
        };
        self.build(args.word(0)?)?.params.push(argument);
        Ok(0)
    }

    fn open(
        &mut self,
        args: &Args,
        name: &str,
        stage: Option<Stage>,
        result: usize,
    ) -> Result<u64, String> {
        let module = args.index(0)?;
        let result = match args.optional(result)? {
            None => None,
            Some(ty) => Some(FunctionResult {
                ty,
                binding: fragment::binding(args.text(args.texts.len() - 1)?)?,
            }),
        };
        let build = self.build(args.word(0)?)?;
        let span = build.span;
        let arguments = std::mem::take(&mut build.params);
        let builder = match stage {
            Some(stage) => build
                .builder
                .entry_point(name, stage, arguments, result, span),
            None => build.builder.function(name, arguments, result, span),
        };
        self.functions.push(Some(PendingFunction {
            module,
            builder,
            workgroup: [WorkgroupExtent::Fixed(1); 3],
            sized: false,
        }));
        word(self.functions.len() - 1)
    }

    /// `klf.shader.function(module, name, result, result binding)`: opens a
    /// function over the pending parameters; `result` `-1` returns nothing.
    pub(super) fn function(&mut self, args: &Args) -> Result<u64, String> {
        let name = args.text(0)?.to_owned();
        self.open(args, &name, None, 1)
    }

    /// `klf.shader.entry(module, name, stage, result, result binding)`.
    pub(super) fn entry(&mut self, args: &Args) -> Result<u64, String> {
        let name = args.text(0)?.to_owned();
        let stage = Stage::from_name(args.text(1)?)
            .ok_or_else(|| format!("unknown stage `{}`", args.text(1).unwrap_or_default()))?;
        self.open(args, &name, Some(stage), 1)
    }

    pub(super) fn pending(
        &mut self,
        module: u64,
        function: u64,
    ) -> Result<&mut PendingFunction, String> {
        let module = usize::try_from(module).map_err(|_| "unknown shader module".to_owned())?;
        usize::try_from(function)
            .ok()
            .and_then(|index| self.functions.get_mut(index))
            .and_then(Option::as_mut)
            .filter(|pending| pending.module == module)
            .ok_or_else(|| "a function builder that is not open in this module".to_owned())
    }

    /// `klf.shader.entry.workgroup(module, entry, x, y, z)`.
    pub(super) fn entry_workgroup(&mut self, args: &Args) -> Result<u64, String> {
        let extents = [args.small(2)?, args.small(3)?, args.small(4)?];
        let pending = self.pending(args.word(0)?, args.word(1)?)?;
        pending.workgroup = extents.map(WorkgroupExtent::Fixed);
        pending.sized = true;
        Ok(0)
    }

    /// `klf.shader.entry.workgroup_override(module, entry, axis, expression)`:
    /// one extent given by an override expression.
    pub(super) fn entry_workgroup_override(&mut self, args: &Args) -> Result<u64, String> {
        let axis = args.index(2)?;
        let expression = args.handle(3)?;
        let pending = self.pending(args.word(0)?, args.word(1)?)?;
        let slot = pending
            .workgroup
            .get_mut(axis)
            .ok_or_else(|| format!("workgroup axis {axis}"))?;
        *slot = WorkgroupExtent::Override(expression);
        pending.sized = true;
        Ok(0)
    }

    /// `klf.shader.function.finish(module, function)`: the function's
    /// handle, or an entry point's index.
    pub(super) fn function_finish(&mut self, args: &Args) -> Result<u64, String> {
        self.pending(args.word(0)?, args.word(1)?)?;
        let pending = self.functions[args.index(1)?]
            .take()
            .ok_or_else(|| "a finished function".to_owned())?;
        let build = self.build(args.word(0)?)?;
        if pending.builder.stage().is_some() {
            let compute = pending.builder.stage() == Some(Stage::Compute);
            if compute && !pending.sized {
                return Err("a compute entry point without a workgroup size".to_owned());
            }
            let size = compute.then_some(pending.workgroup);
            let index = build
                .builder
                .add_entry_point(pending.builder, size)
                .map_err(|error| error.0)?;
            return word(index);
        }
        let handle = build
            .builder
            .add_function(pending.builder)
            .map_err(|error| error.0)?;
        word(handle.raw())
    }

    /// `klf.shader.group(module, name, class)`: opens a resource group of
    /// the shader declaration.
    pub(super) fn group(&mut self, args: &Args) -> Result<u64, String> {
        let class = GroupClass::from_name(args.text(1)?)
            .ok_or_else(|| format!("unknown group class `{}`", args.text(1).unwrap_or_default()))?;
        let group = GroupDecl {
            name: args.text(0)?.to_owned(),
            class,
            globals: Vec::new(),
        };
        self.build(args.word(0)?)?.groups.push(group);
        Ok(0)
    }

    /// `klf.shader.group.add(module, global)`.
    pub(super) fn group_add(&mut self, args: &Args) -> Result<u64, String> {
        let global = args.handle(1)?;
        let build = self.build(args.word(0)?)?;
        build
            .groups
            .last_mut()
            .ok_or_else(|| "a global added before any group".to_owned())?
            .globals
            .push(global);
        Ok(0)
    }

    /// `klf.shader.option(module, override)`: one of the shader's options.
    pub(super) fn option(&mut self, args: &Args) -> Result<u64, String> {
        let option = args.handle(1)?;
        let build = self.build(args.word(0)?)?;
        if build.builder.module.overrides.get(option).is_none() {
            return Err("an option that is not an override".to_owned());
        }
        build.options.push(option);
        Ok(0)
    }

    /// `klf.shader.shader(module, name, kind)`: the shader of the pending
    /// entry indices, groups and options.
    pub(super) fn shader(&mut self, args: &Args) -> Result<u64, String> {
        let kind = ShaderKind::from_name(args.text(1)?)
            .ok_or_else(|| format!("unknown shader kind `{}`", args.text(1).unwrap_or_default()))?;
        let build = self.build(args.word(0)?)?;
        let span = build.span;
        let entries = std::mem::take(&mut build.words)
            .into_iter()
            .map(|entry| usize::try_from(entry).map_err(|_| "an entry index".to_owned()))
            .collect::<Result<Vec<_>, _>>()?;
        let groups = std::mem::take(&mut build.groups);
        let options = std::mem::take(&mut build.options);
        build
            .builder
            .shader(args.text(0)?, kind, entries, groups, options, span)
            .map_err(|error| error.0)?;
        Ok(0)
    }
}
