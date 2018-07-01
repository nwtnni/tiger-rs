use itertools::Itertools;
use itertools::FoldWhile::{Continue, Done};
use sym::store;
use uuid::Uuid;

use ast::*;
use ir;

use check::TypeContext;
use config::WORD_SIZE;
use operand::{Temp, Reg};
use translate::{Call, Frame, FnContext};
use ty::Ty;

pub struct Translator {
    data: Vec<ir::Static>,
    done: Vec<Frame>,
    loops: Vec<ir::Label>,
    frames: Vec<Frame>,
    fc: FnContext,
    tc: TypeContext,
}

impl Translator {

    pub fn translate(ast: &Exp) -> (Vec<ir::Static>, Vec<Frame>) {
        let mut translator = Translator {
            data: Vec::new(),
            done: Vec::new(),
            loops: Vec::new(),
            frames: vec![Frame::new(ir::Label::from_fixed("main"), Vec::new())],
            fc: FnContext::default(),
            tc: TypeContext::default(),
        };

        let main_exp = translator.translate_exp(ast);
        let main_frame = translator.frames.pop()
            .expect("Internal error: missing main frame");

        translator.done.push(
            main_frame.wrap(main_exp)
        );

        (translator.data, translator.done)
    }

    fn translate_var(&mut self, var: &Var) -> (ir::Tree, Ty) {
        match var {
        | Var::Simple(name, span) => {

            // Start off at current frame's base pointer
            let rbp = ir::Exp::Temp(Temp::Reg(Reg::RBP));
            let link = store("STATIC_LINK");

            // Retrieve variable type
            let var_ty = self.tc.get_full(span, name)
                .expect("Internal error: unbound variable");

            // Follow static links
            let var_exp = self.frames.iter().fold_while(rbp, |acc, frame| {
                if frame.contains(*name) {
                    Done(frame.get(*name, acc))
                } else {
                    Continue(frame.get(link, acc))
                }
            }).into_inner();

            (var_exp.into(), var_ty)
        },
        | Var::Field(record, field, _, _) => {

            // Translate record l-value
            let (record_exp, record_type) = self.translate_var(&**record);

            // Find field-type associations
            let fields = match record_type {
            | Ty::Rec(fields, _) => fields,
            | _                  => panic!("Internal error: not a record")
            };

            // Calculate index and type of resulting expression
            let (index, field_ty) = fields.iter()
                .enumerate()
                .find(|(_, (name, _))| field == name)
                .map(|(index, (_, ty))| (index as i32, ty))
                .expect("Internal error: missing field");

            // Calculate memory address offset from record pointer
            let address_exp = ir::Exp::Mem(
                Box::new(
                    ir::Exp::Binop(
                        Box::new(record_exp.into()),
                        ir::Binop::Add,
                        Box::new(ir::Exp::Const(index * WORD_SIZE)),
                    )
                )
            );

            (address_exp.into(), field_ty.clone())
        },
        | Var::Index(array, index, _) => {

            // Translate array l-value
            let (array_exp, array_ty) = self.translate_var(&**array);

            // Find array element type
            let element_ty = match array_ty {
            | Ty::Arr(ty, _) => ty,
            | _              => panic!("Internal error: not an array"),
            };

            // Translate index
            let index_exp = self.translate_exp(&**index);

            // Multiply offset by word size
            let offset_exp = ir::Exp::Binop(
                Box::new(index_exp.into()),
                ir::Binop::Mul,
                Box::new(ir::Exp::Const(WORD_SIZE)),
            );

            // Calculate memory address offset from array pointer
            let address_exp = ir::Exp::Mem(
                Box::new(
                    ir::Exp::Binop(
                        Box::new(array_exp.into()),
                        ir::Binop::Add,
                        Box::new(offset_exp),
                    )
                )
            );

            (address_exp.into(), *element_ty.clone())
        },
        }
    }

    fn translate_exp(&mut self, ast: &Exp) -> ir::Tree {

        match ast {
        | Exp::Break(_) => {

            // Find latest loop exit label on stack
            let label = *self.loops.last()
                .expect("Internal error: break without enclosing loop");

            // Jump to exit label
            ir::Stm::Jump(
                ir::Exp::Name(label),
                vec![label],
            ).into()

        },
        | Exp::Nil(_) => ir::Exp::Const(0).into(),
        | Exp::Var(var, _) => self.translate_var(var).0,
        | Exp::Int(n, _) => ir::Exp::Const(*n).into(),
        | Exp::Str(s, _) => {

            let data = ir::Static::new(s.to_string());
            let label = data.label();
            self.data.push(data);
            ir::Exp::Name(label).into()

        },
        | Exp::Call{name, args, ..} => {

            // Find label from context
            let call = self.fc.get(name);

            let (mut arg_exps, label) = match call {
            | Call::Extern(label) => (Vec::new(), label),
            | Call::Function(label) => (vec![ir::Exp::Temp(Temp::from_reg(Reg::RBP)).into()], label),
            };

            // Translate args sequentially
            arg_exps.extend(
                args.iter()
                    .map(|arg| self.translate_exp(arg))
                    .map(|arg| arg.into())
            );

            // Call function
            ir::Exp::Call(
                Box::new(ir::Exp::Name(label)),
                arg_exps,
            ).into()
        },
        | Exp::Neg(exp, _) => {

            // Subtract sub-expression from 0
            ir::Exp::Binop(
                Box::new(ir::Exp::Const(0)),
                ir::Binop::Sub,
                Box::new(self.translate_exp(exp).into()),
            ).into()

        },
        | Exp::Bin{lhs, op, rhs, ..} => {

            let lhs_exp = self.translate_exp(lhs).into();
            let rhs_exp = self.translate_exp(rhs).into();

            // Straightforward arithmetic operation
            if let Some(binop) = Self::translate_binop(op) {
                ir::Exp::Binop(
                    Box::new(lhs_exp), binop, Box::new(rhs_exp)
                ).into()
            }

            // Conditional operation
            else if let Some(relop) = Self::translate_relop(op) {
                ir::Tree::Cx(
                    Box::new(move |t, f| {
                        ir::Stm::CJump(lhs_exp.clone(), relop, rhs_exp.clone(), t, f)
                    })
                )
            }

            // All operations must be covered
            else {
                panic!("Internal error: non-exhaustive binop check");
            }
        },
        | Exp::Rec{fields, ..} => {

            // Calculate record size for malloc
            let size = ir::Exp::Const(WORD_SIZE * fields.len() as i32);

            // Retrieve malloc label
            let malloc = match self.fc.get(&store("malloc")) {
            | Call::Extern(label) => label,
            | _                   => panic!("Internal error: overridden malloc"),
            };

            // Allocate temp for record pointer
            let pointer = Temp::from_str("MALLOC");

            // Call malloc and move resulting pointer into temp
            let mut seq = vec![
                ir::Stm::Move(
                    ir::Exp::Call(
                        Box::new(ir::Exp::Name(malloc)),
                        vec![size],
                    ),
                    ir::Exp::Temp(pointer),
                ),
            ];

            // Move each field into memory offset from record pointer
            for (i, field) in fields.iter().enumerate() {
                seq.push(
                    ir::Stm::Move(
                        self.translate_exp(&*field.exp).into(),
                        ir::Exp::Mem(
                            Box::new(
                                ir::Exp::Binop(
                                    Box::new(ir::Exp::Temp(pointer)),
                                    ir::Binop::Add,
                                    Box::new(ir::Exp::Const(WORD_SIZE * i as i32)),
                                )
                            )
                        ),
                    )
                );
            }

            // Return record pointer after initialization
            ir::Exp::ESeq(
                Box::new(ir::Stm::Seq(seq)),
                Box::new(ir::Exp::Temp(pointer)),
            ).into()
        },
        | Exp::Seq(exps, _) => {

            // Unit is a no-op
            if exps.is_empty() {
                return ir::Exp::Const(0).into()
            }

            let (last, rest) = exps.split_last().unwrap();

            // Translate last exp into an ir::Exp
            let last_exp = self.translate_exp(last).into();

            // Translate rest of exps into ir::Stm
            let rest_stm = rest.iter()
                .map(|stm| self.translate_exp(stm))
                .map(|stm| stm.into())
                .collect();

            ir::Exp::ESeq(
                Box::new(ir::Stm::Seq(rest_stm)),
                Box::new(last_exp),
            ).into()
        },
        | Exp::Ass{name, exp, ..} => {

            let lhs_exp = self.translate_var(name).0;
            let rhs_exp = self.translate_exp(exp);
            ir::Stm::Move(rhs_exp.into(), lhs_exp.into()).into()

        },
        | Exp::If{guard, then, or, ..} => {

            if let Some(or_exp) = or {

                let t_label = ir::Label::from_str("TRUE_BRANCH");
                let f_label = ir::Label::from_str("FALSE_BRANCH");
                let e_label = ir::Label::from_str("EXIT_IF_ELSE");
                let result = Temp::from_str("IF_ELSE_RESULT");

                ir::Exp::ESeq(
                    Box::new(ir::Stm::Seq(vec![

                        // Evaluate guard expression and jump to correct branch
                        ir::Stm::CJump(
                            self.translate_exp(guard).into(),
                            ir::Relop::Eq,
                            ir::Exp::Const(0),
                            f_label,
                            t_label,
                        ),

                        // Move result of true branch
                        ir::Stm::Label(t_label),
                        ir::Stm::Move(
                            self.translate_exp(then).into(),
                            ir::Exp::Temp(result),
                        ),
                        ir::Stm::Jump(
                            ir::Exp::Name(e_label),
                            vec![e_label],
                        ),

                        // Move result of false branch
                        ir::Stm::Label(f_label),
                        ir::Stm::Move(
                            self.translate_exp(or_exp).into(),
                            ir::Exp::Temp(result),
                        ),
                        ir::Stm::Jump(
                            ir::Exp::Name(e_label),
                            vec![e_label],
                        ),

                        // Exit branch
                        ir::Stm::Label(e_label),
                    ])),
                    Box::new(ir::Exp::Temp(result)),
                ).into()

            } else {

                let t_label = ir::Label::from_str("TRUE_BRANCH");
                let e_label = ir::Label::from_str("EXIT_IF");

                ir::Stm::Seq(vec![

                    // Evaluate guard expression and jumpt to exit if false
                    ir::Stm::CJump(
                        self.translate_exp(guard).into(),
                        ir::Relop::Eq,
                        ir::Exp::Const(0),
                        e_label,
                        t_label,
                    ),

                    // Execute branch
                    ir::Stm::Label(t_label),
                    self.translate_exp(then).into(),
                    ir::Stm::Jump(
                        ir::Exp::Name(e_label),
                        vec![e_label],
                    ),

                    // Skip branch
                    ir::Stm::Label(e_label),
                ]).into()

            }
        },
        | Exp::While{guard, body, ..} => {

            let s_label = ir::Label::from_str("START_WHILE");
            let t_label = ir::Label::from_str("TRUE_BRANCH");
            let e_label = ir::Label::from_str("EXIT_WHILE");

            let guard_exp = self.translate_exp(guard).into();

            // Push exit label of enclosing loop onto context
            self.loops.push(e_label);
            let body_stm = self.translate_exp(body);
            self.loops.pop().expect("Internal error: loop mismatch");

            ir::Stm::Seq(vec![

                // Invariant: all labels must be proceeded by a jump
                ir::Stm::Jump(
                    ir::Exp::Name(s_label),
                    vec![s_label],
                ),

                // While loop header
                ir::Stm::Label(s_label),

                // Evaluate guard expression and jump to exit if false
                ir::Stm::CJump(
                    guard_exp,
                    ir::Relop::Eq,
                    ir::Exp::Const(0),
                    e_label,
                    t_label,
                ),

                // Execute loop body and repeat
                ir::Stm::Label(t_label),
                body_stm.into(),
                ir::Stm::Jump(
                    ir::Exp::Name(s_label),
                    vec![s_label],
                ),

                // Exit loop
                ir::Stm::Label(e_label),

            ]).into()
        },
        | Exp::For{name, escape, lo, hi, body, ..} => {

            let index_location = self.frames.last_mut()
                .expect("Internal error: missing frame")
                .allocate(*name, *escape);

            let s_label = ir::Label::from_str("START_FOR");
            let t_label = ir::Label::from_str("TRUE_BRANCH");
            let e_label = ir::Label::from_str("EXIT_FOR");

            let lo_exp = self.translate_exp(lo);
            let hi_exp = self.translate_exp(hi);

            self.loops.push(s_label);
            let body_stm = self.translate_exp(body);
            self.loops.pop().expect("Internal error: missing for loop");

            ir::Stm::Seq(vec![

                // Initialize index variable
                ir::Stm::Move(
                    lo_exp.into(),
                    index_location.clone(),
                ),

                // Invariant: all labels must be proceeded by a jump
                ir::Stm::Jump(
                    ir::Exp::Name(s_label),
                    vec![s_label],
                ),

                // Loop header
                ir::Stm::Label(s_label),
                ir::Stm::CJump(
                    index_location.clone(),
                    ir::Relop::Gt,
                    hi_exp.into(),
                    e_label,
                    t_label,
                ),

                // True branch: execute body and then increment index
                ir::Stm::Label(t_label),
                body_stm.into(),
                ir::Stm::Move(
                    ir::Exp::Binop(
                        Box::new(index_location.clone()),
                        ir::Binop::Add,
                        Box::new(ir::Exp::Const(1)),
                    ),
                    index_location,
                ),
                ir::Stm::Jump(
                    ir::Exp::Name(s_label),
                    vec![s_label],
                ),

                // Exit label
                ir::Stm::Label(e_label),

            ]).into()
        },
        | Exp::Let{decs, body, ..} => {

            self.tc.push();
            self.fc.push();

            // Translate declarations with side effects
            let mut body_exp = decs.iter()
                .filter_map(|dec| self.translate_dec(&*dec))
                .map(|dec| dec.into())
                .collect::<Vec<_>>();

            // Translate body
            body_exp.push(
                self.translate_exp(&*body).into()
            );

            self.fc.pop();
            self.tc.pop();

            ir::Stm::Seq(body_exp).into()
        }
        | Exp::Arr{size, init, ..} => {

            let size_exp = self.translate_exp(&*size);
            let init_exp = self.translate_exp(&*init);

            let extern_label = match self.fc.get(&store("init_array")) {
            | Call::Extern(label) => label,
            | _                   => panic!("Internal error: overridden init_array"),
            };

            ir::Exp::Call(
                Box::new(ir::Exp::Name(extern_label)),
                vec![
                    size_exp.into(),
                    init_exp.into()
                ],
            ).into()
        },
        }
    }

    fn translate_binop(op: &Binop) -> Option<ir::Binop> {
        match op {
        | Binop::Add  => Some(ir::Binop::Add),
        | Binop::Sub  => Some(ir::Binop::Sub),
        | Binop::Mul  => Some(ir::Binop::Mul),
        | Binop::Div  => Some(ir::Binop::Div),
        | Binop::LAnd => Some(ir::Binop::And),
        | Binop::LOr  => Some(ir::Binop::Or),
        _ => None,
        }
    }

    fn translate_relop(op: &Binop) -> Option<ir::Relop> {
        match op {
        | Binop::Eq  => Some(ir::Relop::Eq),
        | Binop::Neq => Some(ir::Relop::Ne),
        | Binop::Lt  => Some(ir::Relop::Lt),
        | Binop::Le  => Some(ir::Relop::Le),
        | Binop::Gt  => Some(ir::Relop::Gt),
        | Binop::Ge  => Some(ir::Relop::Ge),
        _ => None,
        }
    }

    fn translate_dec(&mut self, dec: &Dec) -> Option<ir::Tree> {
        match dec {
        | Dec::Fun(funs, _) => {

            for fun in funs {

                // Set up static link as first argument
                let mut args = vec![
                    (store("STATIC_LINK"), true)
                ];

                // Collect arg names and escapes
                args.extend(
                    fun.args
                        .iter()
                        .map(|arg| (arg.name, arg.escape))
                );

                // Create new frame
                let label = self.fc.insert(fun.name);
                let frame = Frame::new(label, args);

                // Translate body with new frame
                self.frames.push(frame);
                let body_exp = self.translate_exp(&fun.body);
                let frame = self.frames.pop()
                    .expect("Internal error: missing frame");

                // Push finished function to done pile
                self.done.push(frame.wrap(body_exp));

            }

            None

        },
        | Dec::Var{name, escape, init, ..} => {

            let init_exp = self.translate_exp(init);
            let name_exp = self.frames.last_mut()
                .expect("Internal error: missing frame")
                .allocate(*name, *escape);

            Some(
                ir::Stm::Move(
                    init_exp.into(),
                    name_exp.into(),
                ).into()
            )
        }
        | Dec::Type(decs, _) => {

            for dec in decs {
                self.tc.insert(dec.name, Ty::Name(dec.name, None));
            }

            for dec in decs {
                let ty = Box::new(self.translate_type(&dec.ty));
                self.tc.insert(dec.name, Ty::Name(dec.name, Some(ty)));
            }

            None
        },
        }
    }

    fn translate_type(&mut self, ty: &Type) -> Ty {

        match ty {
        | Type::Name(name, span) => {

            self.tc.get_partial(span, name)
                .expect("Internal error: missing name type")

        },
        | Type::Arr(name, name_span, span) => {

            // Look up array element type
            let elem_ty = self.tc.get_partial(name_span, name)
                .expect("Internal error: missing element type");

            Ty::Arr(Box::new(elem_ty), Uuid::new_v4())

        },
        | Type::Rec(decs, _) => {

            let mut fields = Vec::new();

            // Look up each field type
            for dec in decs {
                let field_ty = self.tc.get_partial(&dec.ty_span, &dec.ty)
                    .expect("Internal error: missing field type");
                fields.push((dec.name, field_ty));
            }

            Ty::Rec(fields, Uuid::new_v4())
        },
        }

    }
}
