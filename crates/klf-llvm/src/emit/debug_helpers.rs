use super::*;

impl FnEmitter<'_, '_> {
    /// The `!dbg` a definition, an instruction or a label carries. A line
    /// indented by two spaces is an instruction inside a block and takes the
    /// location of the instruction being lowered; anything else is structure.
    pub(super) fn dbg(&self) -> String {
        self.location
            .map_or_else(String::new, |node| format!(", !dbg !{node}"))
    }

    /// The `!dbg` a definition carries, which is its subprogram.
    pub(super) fn subprogram_dbg(&self) -> String {
        self.subprogram
            .map_or_else(String::new, |node| format!("!dbg !{node} "))
    }

    pub(super) fn at(&mut self, location: Option<&DebugLocation>) {
        self.location = match (self.subprogram, location) {
            (Some(_), Some(location)) => {
                Some(self.debug.location(self.function.name.as_str(), location))
            }
            _ => None,
        };
    }

    /// Places the following records at the function's declaration site.
    pub(super) fn at_declaration(&mut self, owner: &str) {
        self.location = Some(self.debug.declaration(owner));
    }

    /// Names `parameter` so that a debugger prints it (section 16.3). The
    /// slot the entry block spilled it into is the stable home a `#dbg_declare`
    /// wants, where an argument register does not survive the first call.
    pub(super) fn name_parameter(&mut self, index: u32, parameter: &klf_kir::Parameter) {
        let name = self.function.name.as_str().to_owned();
        let Some(variable) =
            self.debug
                .parameter(&name, parameter.name.as_str(), &parameter.ty, index)
        else {
            return;
        };
        let slot = format!("ptr %\"ps.{}\"", parameter.name.as_str());
        self.at_declaration(&name);
        self.debug_record("dbg_declare", &slot, variable);
        self.at(None);
    }

    pub(super) fn debug_value(
        &mut self,
        value: &Name,
        variable: &klf_kir::DebugVariable,
    ) -> Result<(), EmitError> {
        let owner = self.function.name.as_str().to_owned();
        let Some(node) =
            self.debug
                .value_variable(&owner, &variable.name, &variable.ty, &variable.scope)
        else {
            return Ok(());
        };
        let ty = llvm_type(&self.ty_of(value)?.clone())?;
        let operand = self.value(value)?;
        self.debug_record("dbg_value", &format!("{ty} {operand}"), node);
        Ok(())
    }

    pub(super) fn debug_address(
        &mut self,
        address: &Name,
        variable: &klf_kir::DebugVariable,
    ) -> Result<(), EmitError> {
        let owner = self.function.name.as_str().to_owned();
        let Some(node) =
            self.debug
                .address_variable(&owner, &variable.name, &variable.ty, &variable.scope)
        else {
            return Ok(());
        };
        let operand = self.value(address)?;
        self.debug_record("dbg_declare", &format!("ptr {operand}"), node);
        Ok(())
    }
}
