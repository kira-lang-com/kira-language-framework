use super::*;

impl Ir<'_> {
    /// The function adopting or copying a value of `ty` at an address.
    ///
    /// # Errors
    /// Returns the layout engine's error when `ty` has no layout.
    pub fn adopter(&mut self, ty: &Type, mode: Mode) -> Result<String, String> {
        let key = (type_key(ty), mode);
        if let Some(name) = self.adopters.get(&key) {
            return Ok(name.clone());
        }
        let name = format!(
            "ridl.{}.{}",
            match mode {
                Mode::Move => "adopt",
                Mode::Copy => "copy",
                Mode::Unique => "unique",
            },
            key.0
        );
        self.adopters.insert(key, name.clone());
        let mut body = Body::new();
        match ty {
            Type::String if mode == Mode::Unique => self.unique_buffer(&mut body, None)?,
            Type::Array(element) if mode == Mode::Unique => {
                self.unique_buffer(&mut body, Some(element))?;
            }
            Type::TraitObject(_) if mode == Mode::Copy => self.copy_box(&mut body, 0),
            Type::Function { .. } if mode == Mode::Copy => self.copy_box(&mut body, self.word),
            Type::BorrowFunction { .. } if mode == Mode::Copy => body.line("ret void"),
            Type::String => self.adopt_buffer(&mut body, None, mode)?,
            Type::Array(element) => self.adopt_buffer(&mut body, Some(element), mode)?,
            Type::Tuple(items) => {
                let (_, placements) = self
                    .engine
                    .aggregate(items)
                    .map_err(|error| format!("{error:?}"))?;
                let parts: Vec<(Type, u64)> = items
                    .iter()
                    .cloned()
                    .zip(placements.into_iter().map(|placement| placement.offset))
                    .collect();
                self.adopt_parts(&mut body, &parts, mode)?;
                body.line("ret void");
            }
            Type::Nominal {
                name: type_name,
                args,
            } if matches!(self.registry.get(type_name), Some(Decl::Struct(_))) => {
                let parts = self.fields(type_name, args)?;
                self.adopt_parts(&mut body, &parts, mode)?;
                body.line("ret void");
            }
            Type::Optional(_) | Type::Result(..) | Type::Nominal { .. } => {
                self.adopt_enum(&mut body, ty, mode)?;
            }
            other => {
                return Err(format!(
                    "RIDL entries cannot take over a value of `{}`",
                    type_key(other)
                ));
            }
        }
        let _ = write!(
            self.functions,
            "define private void {}(ptr %value) {{\nentry:\n{}}}\n\n",
            quote(&name),
            body.text
        );
        Ok(name)
    }

    fn adopt_parts(
        &mut self,
        body: &mut Body,
        parts: &[(Type, u64)],
        mode: Mode,
    ) -> Result<(), String> {
        for (ty, offset) in parts {
            if self.reaches(ty, mode) {
                let adopter = self.adopter(ty, mode)?;
                let address = body.at("%value", *offset);
                body.line(&format!("call void {}(ptr {address})", quote(&adopter)));
            }
        }
        Ok(())
    }

    fn adopt_enum(&mut self, body: &mut Body, ty: &Type, mode: Mode) -> Result<(), String> {
        let shape = self
            .engine
            .enum_shape(ty)
            .map_err(|error| format!("{error:?}"))?
            .ok_or_else(|| format!("`{}` is not an enum", type_key(ty)))?;
        if shape.niche {
            // The payload-less case is a zero first word; the other case's
            // one payload starts at offset 0 (section 21.3.4).
            let payload = shape.cases.iter().find(|parts| !parts.is_empty());
            if let Some(parts) =
                payload.filter(|parts| parts.iter().any(|(ty, _)| self.reaches(ty, mode)))
            {
                let word = body.load("ptr", "%value");
                let absent = body.value();
                body.line(&format!("{absent} = icmp eq ptr {word}, null"));
                body.line(&format!("br i1 {absent}, label %done, label %present"));
                body.block("present");
                self.adopt_parts(body, parts, mode)?;
                body.line("br label %done");
                body.block("done");
            }
            body.line("ret void");
            return Ok(());
        }
        let tag_type = format!("i{}", shape.tag_size * 8);
        let tag = body.load(&tag_type, "%value");
        let mut cases = Vec::new();
        for (index, payload) in shape.cases.iter().enumerate() {
            if payload.iter().any(|(ty, _)| self.reaches(ty, mode)) {
                cases.push((index, body.label("case")));
            }
        }
        let arms: Vec<String> = cases
            .iter()
            .map(|(index, label)| format!("{tag_type} {index}, label %{label}"))
            .collect();
        body.line(&format!(
            "switch {tag_type} {tag}, label %done [ {} ]",
            arms.join(" ")
        ));
        for (index, label) in cases {
            body.block(&label);
            self.adopt_parts(body, &shape.cases[index], mode)?;
            body.line("br label %done");
        }
        body.block("done");
        body.line("ret void");
        Ok(())
    }

    /// Gives the box at `offset` in `%value` a holder of its own through the
    /// frontend's `BOX_COPY`: the box of an `Any Trait`, or the context of a
    /// closure, which may be null.
    fn copy_box(&mut self, body: &mut Body, offset: u64) {
        let copy = quote(&(self.defined)(BOX_COPY));
        self.declare(BOX_COPY, &format!("declare ptr {copy}(ptr)"));
        let at = body.at("%value", offset);
        let boxed = body.load("ptr", &at);
        let absent = body.value();
        body.line(&format!("{absent} = icmp eq ptr {boxed}, null"));
        body.line(&format!("br i1 {absent}, label %done, label %held"));
        body.block("held");
        let copied = body.value();
        body.line(&format!("{copied} = call ptr {copy}(ptr {boxed})"));
        body.store("ptr", &copied, &at);
        body.line("br label %done");
        body.block("done");
        body.line("ret void");
    }

    /// Adopts or copies the buffer of a `String` or `[T]` at `%value`.
    fn adopt_buffer(
        &mut self,
        body: &mut Body,
        element: Option<&Type>,
        mode: Mode,
    ) -> Result<(), String> {
        let word = self.word;
        let iw = self.iw();
        let (stride, align) = match element {
            Some(element) => {
                let (size, align) = self.size_align(element)?;
                (round_up(size, align), align)
            }
            None => (1, 1),
        };
        let data = body.load("ptr", "%value");
        let length_at = body.at("%value", word);
        let length = body.load(&iw, &length_at);
        let capacity_at = body.at("%value", 2 * word);
        let capacity = body.load(&iw, &capacity_at);
        let ops_at = body.at("%value", 3 * word);
        let ops = body.load("ptr", &ops_at);
        let own_ops = body.value();
        body.line(&format!(
            "{own_ops} = call ptr {}()",
            quote(&(self.defined)(BUFFER_OPS))
        ));
        if mode == Mode::Move {
            let own = body.value();
            body.line(&format!("{own} = icmp eq ptr {ops}, {own_ops}"));
            body.line(&format!("br i1 {own}, label %elements, label %foreign"));
            body.block("foreign");
        } else {
            body.line("br label %foreign");
            body.block("foreign");
        }
        let bytes = body.value();
        body.line(&format!("{bytes} = mul {iw} {length}, {stride}"));
        let bytes64 = if word == 8 {
            bytes.clone()
        } else {
            let wide = body.value();
            body.line(&format!("{wide} = zext {iw} {bytes} to i64"));
            wide
        };
        let fresh = body.value();
        body.line(&format!(
            "{fresh} = call ptr {}(i64 {bytes64})",
            quote(&(self.defined)(BUFFER_ALLOCATE))
        ));
        body.line(&format!(
            "call void @llvm.memcpy.p0.p0.{iw}(ptr {fresh}, ptr {data}, {iw} {bytes}, i1 false)"
        ));
        self.declare(
            "llvm.memcpy",
            &format!("declare void @llvm.memcpy.p0.p0.{iw}(ptr, ptr, {iw}, i1)"),
        );
        if mode == Mode::Move {
            let live = body.value();
            body.line(&format!("{live} = icmp ne ptr {ops}, null"));
            body.line(&format!("br i1 {live}, label %release, label %replace"));
            body.block("release");
            let free_at = body.at(&ops, 8 + word);
            let free = body.load("ptr", &free_at);
            body.line(&format!(
                "call void {free}(ptr {data}, {iw} {capacity}, {iw} {stride}, {iw} {align})"
            ));
            body.line("br label %replace");
        } else {
            body.line("br label %replace");
        }
        body.block("replace");
        body.store("ptr", &fresh, "%value");
        body.store(&iw, &length, &capacity_at);
        body.store("ptr", &own_ops, &ops_at);
        body.line("br label %elements");
        body.block("elements");
        match element.filter(|element| self.reaches(element, mode)) {
            Some(element) => {
                let adopter = self.adopter(element, mode)?;
                body.line("br label %loop");
                body.block("loop");
                body.line(&format!(
                    "%index = phi {iw} [ 0, %elements ], [ %following, %each ]"
                ));
                body.line(&format!("%finished = icmp eq {iw} %index, {length}"));
                body.line("br i1 %finished, label %done, label %each");
                body.block("each");
                let current = body.load("ptr", "%value");
                let offset = body.value();
                body.line(&format!("{offset} = mul {iw} %index, {stride}"));
                let item = body.value();
                body.line(&format!(
                    "{item} = getelementptr inbounds i8, ptr {current}, {iw} {offset}"
                ));
                body.line(&format!("call void {}(ptr {item})", quote(&adopter)));
                body.line(&format!("%following = add {iw} %index, 1"));
                body.line("br label %loop");
                body.block("done");
                body.line("ret void");
            }
            None => body.line("ret void"),
        }
        Ok(())
    }
}

impl Ir<'_> {
    /// Makes the buffer of the `String` or `[T]` at `%value` the frontend's
    /// alone, with its elements. A buffer of its own that nothing else holds
    /// is kept, and its elements made unique in turn. One it shares is copied
    /// and its elements copied with it before the shared one is released, so
    /// what the other holders see is untouched. One another allocator made is
    /// taken over as `Mode::Move` takes it.
    fn unique_buffer(&mut self, body: &mut Body, element: Option<&Type>) -> Result<(), String> {
        let word = self.word;
        let iw = self.iw();
        let (stride, align) = match element {
            Some(element) => {
                let (size, align) = self.size_align(element)?;
                (round_up(size, align), align)
            }
            None => (1, 1),
        };
        let data = body.load("ptr", "%value");
        let length_at = body.at("%value", word);
        let length = body.load(&iw, &length_at);
        let capacity_at = body.at("%value", 2 * word);
        let capacity = body.load(&iw, &capacity_at);
        let ops_at = body.at("%value", 3 * word);
        let ops = body.load("ptr", &ops_at);
        let own_ops = body.value();
        body.line(&format!(
            "{own_ops} = call ptr {}()",
            quote(&(self.defined)(BUFFER_OPS))
        ));
        // A buffer with no capacity has no ops and nothing in it.
        let empty = body.value();
        body.line(&format!("{empty} = icmp eq ptr {ops}, null"));
        body.line(&format!("br i1 {empty}, label %done, label %held"));
        body.block("held");
        let free_at = body.at(&ops, 8 + word);
        let free = body.load("ptr", &free_at);
        let release =
            format!("call void {free}(ptr {data}, {iw} {capacity}, {iw} {stride}, {iw} {align})");
        let own = body.value();
        body.line(&format!("{own} = icmp eq ptr {ops}, {own_ops}"));
        body.line(&format!("br i1 {own}, label %own, label %taken"));
        body.block("own");
        let unique = body.value();
        body.line(&format!(
            "{unique} = call i1 {}(ptr {data})",
            quote(&(self.defined)(BUFFER_UNIQUE))
        ));
        body.line(&format!("br i1 {unique}, label %kept, label %shared"));
        // Shared: the elements are copied out of it while it is still held.
        body.block("shared");
        self.fresh_buffer(body, &data, &length, stride, &own_ops);
        self.element_loop(body, element, Mode::Copy, "copied", &length, stride)?;
        body.line(&release);
        body.line("br label %done");
        // Another allocator's: taken over, its elements with it.
        body.block("taken");
        self.fresh_buffer(body, &data, &length, stride, &own_ops);
        body.line(&release);
        self.element_loop(body, element, Mode::Move, "adopted", &length, stride)?;
        body.line("br label %done");
        body.block("kept");
        self.element_loop(body, element, Mode::Unique, "kept", &length, stride)?;
        body.line("br label %done");
        body.block("done");
        body.line("ret void");
        Ok(())
    }

    /// Replaces the buffer at `%value` with a copy of its `length` elements of
    /// `stride` bytes in one of the frontend's own.
    fn fresh_buffer(
        &mut self,
        body: &mut Body,
        data: &str,
        length: &str,
        stride: u64,
        own_ops: &str,
    ) {
        let word = self.word;
        let iw = self.iw();
        let bytes = body.value();
        body.line(&format!("{bytes} = mul {iw} {length}, {stride}"));
        let bytes64 = if word == 8 {
            bytes.clone()
        } else {
            let wide = body.value();
            body.line(&format!("{wide} = zext {iw} {bytes} to i64"));
            wide
        };
        let fresh = body.value();
        body.line(&format!(
            "{fresh} = call ptr {}(i64 {bytes64})",
            quote(&(self.defined)(BUFFER_ALLOCATE))
        ));
        body.line(&format!(
            "call void @llvm.memcpy.p0.p0.{iw}(ptr {fresh}, ptr {data}, {iw} {bytes}, i1 false)"
        ));
        self.declare(
            "llvm.memcpy",
            &format!("declare void @llvm.memcpy.p0.p0.{iw}(ptr, ptr, {iw}, i1)"),
        );
        body.store("ptr", &fresh, "%value");
        let capacity_at = body.at("%value", 2 * word);
        body.store(&iw, length, &capacity_at);
        let ops_at = body.at("%value", 3 * word);
        body.store("ptr", own_ops, &ops_at);
    }

    /// Runs `mode`'s adopter over each of the `length` elements of the buffer
    /// at `%value`, in blocks named after `label`; nothing for elements that
    /// hold no buffer.
    fn element_loop(
        &mut self,
        body: &mut Body,
        element: Option<&Type>,
        mode: Mode,
        label: &str,
        length: &str,
        stride: u64,
    ) -> Result<(), String> {
        let Some(element) = element.filter(|element| self.reaches(element, mode)) else {
            return Ok(());
        };
        let iw = self.iw();
        let adopter = self.adopter(element, mode)?;
        body.line(&format!("br label %{label}.entry"));
        body.block(&format!("{label}.entry"));
        body.line(&format!("br label %{label}.loop"));
        body.block(&format!("{label}.loop"));
        body.line(&format!(
            "%{label}.index = phi {iw} [ 0, %{label}.entry ], [ %{label}.following, %{label}.each ]"
        ));
        body.line(&format!(
            "%{label}.finished = icmp eq {iw} %{label}.index, {length}"
        ));
        body.line(&format!(
            "br i1 %{label}.finished, label %{label}.done, label %{label}.each"
        ));
        body.block(&format!("{label}.each"));
        let current = body.load("ptr", "%value");
        let offset = body.value();
        body.line(&format!("{offset} = mul {iw} %{label}.index, {stride}"));
        let item = body.value();
        body.line(&format!(
            "{item} = getelementptr inbounds i8, ptr {current}, {iw} {offset}"
        ));
        body.line(&format!("call void {}(ptr {item})", quote(&adopter)));
        body.line(&format!("%{label}.following = add {iw} %{label}.index, 1"));
        body.line(&format!("br label %{label}.loop"));
        body.block(&format!("{label}.done"));
        Ok(())
    }
}
