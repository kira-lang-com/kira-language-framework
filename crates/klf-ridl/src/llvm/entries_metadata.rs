use super::Ir;
use crate::module::Module;
use std::fmt::Write as _;

impl Ir<'_> {
    pub(super) fn metadata(&mut self, module: &Module, target: &str, require_runtime: bool) {
        let metadata =
            crate::meta::Metadata::for_module(module, Vec::new(), Vec::new(), Vec::new()).encode();
        let section = if target.contains("apple") || target.contains("darwin") {
            "__DATA,__ridlmeta"
        } else if target.contains("windows") {
            ".ridlmt"
        } else {
            ".ridlmeta"
        };
        let bytes = metadata.iter().fold(String::new(), |mut bytes, byte| {
            let _ = write!(bytes, "\\{byte:02X}");
            bytes
        });
        let length = metadata.len();
        let iw = self.iw();
        let _ = writeln!(
            self.globals,
            "@ridl.metadata = private constant [{length} x i8] c\"{bytes}\", section \"{section}\", align 1"
        );
        let _ = writeln!(
            self.globals,
            "@llvm.used = appending global [1 x ptr] [ptr @ridl.metadata], section \"llvm.metadata\""
        );
        let _ = writeln!(
            self.globals,
            "@llvm.global_ctors = appending global [1 x {{ i32, ptr, ptr }}] [{{ i32, ptr, ptr }} {{ i32 65535, ptr @ridl.register, ptr null }}]"
        );
        if require_runtime {
            self.declare(
                "klfrt_ridl_register",
                &format!("declare void @klfrt_ridl_register(ptr, {iw})"),
            );
            let _ = write!(
                self.functions,
                "define private void @ridl.register() {{\nentry:\n  call void @klfrt_ridl_register(ptr @ridl.metadata, {iw} {length})\n  ret void\n}}\n\n"
            );
        } else {
            self.declare(
                "klfrt_ridl_register",
                &format!("declare extern_weak void @klfrt_ridl_register(ptr, {iw})"),
            );
            let _ = write!(
                self.functions,
                "define private void @ridl.register() {{\nentry:\n  %linked = icmp ne ptr @klfrt_ridl_register, null\n  br i1 %linked, label %register, label %done\nregister:\n  call void @klfrt_ridl_register(ptr @ridl.metadata, {iw} {length})\n  br label %done\ndone:\n  ret void\n}}\n\n"
            );
        }
    }
}
