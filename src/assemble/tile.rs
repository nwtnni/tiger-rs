use simple_symbol::store;

use asm;
use asm::Value;
use config::WORD_SIZE;
use ir;
use ir::*;
use operand::*;

pub fn tile(ir: ir::Unit) -> asm::Unit<Temp> {

    let mut tiler = Tiler::default();
    for stm in &ir.body { tiler.tile_stm(stm); }

    let store_rbx = Temp::from_str("STORE_RBX");
    let store_r12 = Temp::from_str("STORE_R12");
    let store_r13 = Temp::from_str("STORE_R13");
    let store_r14 = Temp::from_str("STORE_R14");
    let store_r15 = Temp::from_str("STORE_R15");

    let prologue = vec![
        asm::Asm::Push(asm::Unary::R(Temp::Reg(Reg::RBP))),
        asm::Asm::Mov(asm::Binary::RR(Temp::Reg(Reg::RSP), Temp::Reg(Reg::RBP))),
        asm::Asm::Comment(store("REPLACE WITH RSP SUBTRACTION")),
        asm::Asm::Mov(asm::Binary::RR(Temp::Reg(Reg::RBX), store_rbx)),
        asm::Asm::Mov(asm::Binary::RR(Temp::Reg(Reg::R12), store_r12)),
        asm::Asm::Mov(asm::Binary::RR(Temp::Reg(Reg::R13), store_r13)),
        asm::Asm::Mov(asm::Binary::RR(Temp::Reg(Reg::R14), store_r14)),
        asm::Asm::Mov(asm::Binary::RR(Temp::Reg(Reg::R15), store_r15)),
    ];

    let epilogue = vec![
        asm::Asm::Mov(asm::Binary::RR(store_rbx, Temp::Reg(Reg::RBX))),
        asm::Asm::Mov(asm::Binary::RR(store_r12, Temp::Reg(Reg::R12))),
        asm::Asm::Mov(asm::Binary::RR(store_r13, Temp::Reg(Reg::R13))),
        asm::Asm::Mov(asm::Binary::RR(store_r14, Temp::Reg(Reg::R14))),
        asm::Asm::Mov(asm::Binary::RR(store_r15, Temp::Reg(Reg::R15))),
        asm::Asm::Comment(store("REPLACE WITH RSP ADDITION")),
        asm::Asm::Mov(asm::Binary::RR(Temp::Reg(Reg::RBP), Temp::Reg(Reg::RSP))),
        asm::Asm::Pop(asm::Unary::R(Temp::Reg(Reg::RBP))),
        asm::Asm::Ret,
    ];

    asm::Unit {
        asm: prologue.into_iter()
            .chain(tiler.asm.into_iter())
            .chain(epilogue.into_iter())
            .collect()
    }
}

#[derive(Default)]
struct Tiler {
    asm: Vec<asm::Asm<Temp>>,
    spilled_args: usize,
}

impl Tiler {

    fn into_temp(&mut self, value: Value<Temp>) -> Temp {
        match value {
        | Value::Reg(temp) => temp,
        | Value::Mem(mem) => {
            let temp = Temp::from_str("TILE_MEM");
            self.asm.push(asm::Asm::Mov(asm::Binary::MR(mem, temp)));
            temp
        }
        | Value::Imm(imm) => {
            let temp = Temp::from_str("TILE_IMM");
            self.asm.push(asm::Asm::Mov(asm::Binary::IR(imm, temp)));
            temp
        }
        | _ => unimplemented!(),
        }
    }

    fn tile_stm(&mut self, stm: &Stm) {
        match stm {
        | Stm::Exp(_) => panic!("Internal error: no Exp statement in canonical IR"),
        | Stm::Seq(_) => panic!("Internal error: no Seq statement in canonical IR"),
        | Stm::Comment(s) => self.asm.push(asm::Asm::Comment(store(s))),
        | Stm::Label(l) => self.asm.push(asm::Asm::Label(*l)),
        | Stm::Jump(Exp::Name(label), _) => self.asm.push(asm::Asm::Jmp(*label)),
        | Stm::Jump(_, _) => panic!("Internal error: can only jump to labels"),
        | Stm::Move(l, r) => {
            let binary = self.tile_binary(l, r);
            self.asm.push(asm::Asm::Mov(binary));
        },
        | Stm::CJump(l, op, r, t, _) => {
            let binary = self.tile_binary(l, r);
            self.asm.push(asm::Asm::Cmp(binary));
            self.asm.push(asm::Asm::Jcc(op.into(), *t));
        },
        }
    }

    fn tile_binary(&mut self, lhs: &Exp, rhs: &Exp) -> asm::Binary<Temp> {
        match (self.tile_exp(lhs), self.tile_exp(rhs)) {
        | (Value::Imm(imm), Value::Reg(temp)) => asm::Binary::IR(imm, temp),
        | (Value::Imm(imm), Value::Mem(mem))  => asm::Binary::IM(imm, mem),
        | (Value::Mem(mem), Value::Reg(temp)) => asm::Binary::MR(mem, temp),
        | (temp, Value::Mem(mem)) => {
            let temp = self.into_temp(temp);
            asm::Binary::RM(temp, mem)
        }
        | (temp_a, temp_b) => {
            let temp_a = self.into_temp(temp_a);
            let temp_b = self.into_temp(temp_b);
            asm::Binary::RR(temp_a, temp_b)
        }
        }
    }

    fn tile_exp(&mut self, exp: &Exp) -> Value<Temp> {

        use ir::Exp::{Binop, Const};

        match exp {
        | Exp::Const(n) => Value::Imm(Imm(*n)),
        | Exp::Name(l)  => Value::Label(*l),
        | Exp::Temp(t)  => Value::Reg(*t),
        | Exp::ESeq(_, _) => panic!("Internal error: no ESeq expression in canonical IR"),

        // BRSO memory addressing
        | Exp::Mem(box Binop(box Binop(box b, ir::Binop::Add, box Binop(box r, ir::Binop::Mul, box Const(s))), ir::Binop::Add, box Const(o)))
        | Exp::Mem(box Binop(box Binop(box b, ir::Binop::Add, box Binop(box Const(s), ir::Binop::Mul, box r)), ir::Binop::Add, box Const(o)))
        | Exp::Mem(box Binop(box Binop(box Binop(box r, ir::Binop::Mul, box Const(s)), ir::Binop::Add, box b), ir::Binop::Add, box Const(o)))
        | Exp::Mem(box Binop(box Binop(box Binop(box Const(s), ir::Binop::Mul, box r), ir::Binop::Add, box b), ir::Binop::Add, box Const(o)))
        | Exp::Mem(box Binop(box Const(o), ir::Binop::Add, box Binop(box b, ir::Binop::Add, box Binop(box r, ir::Binop::Mul, box Const(s)))))
        | Exp::Mem(box Binop(box Const(o), ir::Binop::Add, box Binop(box b, ir::Binop::Add, box Binop(box Const(s), ir::Binop::Mul, box r))))
        | Exp::Mem(box Binop(box Const(o), ir::Binop::Add, box Binop(box Binop(box r, ir::Binop::Mul, box Const(s)), ir::Binop::Add, box b)))
        | Exp::Mem(box Binop(box Const(o), ir::Binop::Add, box Binop(box Binop(box Const(s), ir::Binop::Mul, box r), ir::Binop::Add, box b))) => {
            let b = self.tile_exp(b);
            let r = self.tile_exp(r);
            Value::Mem(Mem::BRSO(
                self.into_temp(b),
                self.into_temp(r),
                Scale::try_from(*s),
                *o,
            ))
        },
        | Exp::Mem(box Binop(box Binop(box b, ir::Binop::Add, box Binop(box r, ir::Binop::Mul, box Const(s))), ir::Binop::Sub, box Const(o)))
        | Exp::Mem(box Binop(box Binop(box b, ir::Binop::Add, box Binop(box Const(s), ir::Binop::Mul, box r)), ir::Binop::Sub, box Const(o)))
        | Exp::Mem(box Binop(box Binop(box Binop(box r, ir::Binop::Mul, box Const(s)), ir::Binop::Add, box b), ir::Binop::Sub, box Const(o)))
        | Exp::Mem(box Binop(box Binop(box Binop(box Const(s), ir::Binop::Mul, box r), ir::Binop::Add, box b), ir::Binop::Sub, box Const(o))) => {
            let b = self.tile_exp(b);
            let r = self.tile_exp(r);
            Value::Mem(Mem::BRSO(
                self.into_temp(b),
                self.into_temp(r),
                Scale::try_from(*s),
                -*o,
            ))
        },

        // RSO memory addressing
        | Exp::Mem(box Binop(box Binop(box r, ir::Binop::Mul, box Const(s)), ir::Binop::Add, box Const(o)))
        | Exp::Mem(box Binop(box Binop(box Const(s), ir::Binop::Mul, box r), ir::Binop::Add, box Const(o)))
        | Exp::Mem(box Binop(box Const(o), ir::Binop::Add, box Binop(box r, ir::Binop::Mul, box Const(s))))
        | Exp::Mem(box Binop(box Const(o), ir::Binop::Add, box Binop(box Const(s), ir::Binop::Mul, box r))) => {
            let r = self.tile_exp(r);
            Value::Mem(Mem::RSO(
                self.into_temp(r),
                Scale::try_from(*s),
                *o
            ))
        }
        | Exp::Mem(box Binop(box Binop(box r, ir::Binop::Mul, box Const(s)), ir::Binop::Sub, box Const(o)))
        | Exp::Mem(box Binop(box Binop(box Const(s), ir::Binop::Mul, box r), ir::Binop::Sub, box Const(o))) => {
            let r = self.tile_exp(r);
            Value::Mem(Mem::RSO(
                self.into_temp(r),
                Scale::try_from(*s),
                -*o
            ))
        }

        // RO memory addressing
        | Exp::Mem(box Binop(box r, ir::Binop::Add, box Const(o)))
        | Exp::Mem(box Binop(box Const(o), ir::Binop::Add, box r)) => {
            let r = self.tile_exp(r);
            Value::Mem(Mem::RO(
                self.into_temp(r),
                *o
            ))
        },
        | Exp::Mem(box Binop(box r, ir::Binop::Sub, box Const(o))) => {
            let r = self.tile_exp(r);
            Value::Mem(Mem::RO(
                self.into_temp(r),
                -*o
            ))
        },

        // General memory
        | Exp::Mem(box r) => {
            let r = self.tile_exp(r);
            Value::Mem(Mem::R(
                self.into_temp(r)
            ))
        }

        // Negation
        | Exp::Binop(box Const(0), ir::Binop::Sub, box r) => {
            self.tile_unop(r, asm::Unop::Neg)
        }

        // Increment
        | Exp::Binop(box r, ir::Binop::Add, box Const(1))
        | Exp::Binop(box Const(1), ir::Binop::Add, box r) => {
            self.tile_unop(r, asm::Unop::Inc)
        }

        // Decrement
        | Exp::Binop(box r, ir::Binop::Sub, box Const(1)) => {
            self.tile_unop(r, asm::Unop::Dec)
        }

        // Add, Sub, And, Or, XOr
        | Exp::Binop(box l, op, box r) if op.is_asm_binop() => {
            let binary = self.tile_binary(l, r);
            self.asm.push(asm::Asm::Bin(op.into_asm_binop(), binary));
            binary.dest()
        }

        // Mul, Div
        | Exp::Binop(box l, op, box r) => {

            let l_tile = self.tile_exp(l);
            let r_tile = self.tile_exp(r);
            let rax = Temp::Reg(Reg::RAX);
            let res = Temp::from_str("TILE_DIV_MUL");

            let move_l_tile = match l_tile {
            | Value::Imm(imm) => asm::Binary::IR(imm, rax),
            | Value::Mem(mem) => asm::Binary::MR(mem, rax),
            | temp            => asm::Binary::RR(self.into_temp(temp), rax),
            };

            let use_r_tile = match r_tile {
            | Value::Mem(mem) => asm::Unary::M(mem),
            | temp            => asm::Unary::R(self.into_temp(temp)),
            };

            self.asm.push(asm::Asm::Mov(move_l_tile));

            match op {
            | ir::Binop::Mul => self.asm.push(asm::Asm::Mul(use_r_tile)),
            | ir::Binop::Div => {
                self.asm.push(asm::Asm::Cqo);
                self.asm.push(asm::Asm::Div(use_r_tile));
            }
            | _ => unreachable!(),
            };

            self.asm.push(asm::Asm::Mov(asm::Binary::RR(rax, res)));

            Value::Reg(res)
        }
        | Exp::Call(box Exp::Name(label), args) => {

            let mut arg_offset = 0;
            let return_temp = Temp::from_str("TILE_CALL");

            for (i, arg) in args.into_iter().enumerate() {

                // Dedicated register for first six arguments
                let binary = match self.tile_exp(arg) {
                | Value::Mem(mem) if i < 6 => {
                    asm::Binary::MR(
                        mem,
                        Temp::Reg(Reg::get_argument(i)),
                    )
                }
                | temp if i < 6 => {
                    asm::Binary::RR(
                        self.into_temp(temp),
                        Temp::Reg(Reg::get_argument(i)),
                    )
                }

                // Spill arguments onto stack
                //
                //       ---------
                //       | ARG 9 |
                //       ---------
                //       | ARG 8 |
                //       ---------
                //       | ARG 7 |
                // RSP   ---------
                //       |       |
                //       ---------
                | temp => {

                    let temp = self.into_temp(temp);
                    arg_offset += 1;

                    asm::Binary::RM(
                        temp,
                        Mem::RO(
                            Temp::Reg(Reg::RSP),
                            arg_offset as i32 * WORD_SIZE,
                        ),
                    )
                },
                };

                self.asm.push(asm::Asm::Mov(binary));
            }

            self.spilled_args = usize::max(self.spilled_args, arg_offset);
            self.asm.push(asm::Asm::Call(*label));
            self.asm.push(asm::Asm::Mov(
                asm::Binary::RR(
                    Temp::Reg(Reg::get_return()),
                    return_temp
                )
            ));

            Value::Reg(return_temp)
        }
        | Exp::Call(_, _) => panic!("Internal error: calling non-label"),

        }
    }

    fn tile_unop(&mut self, exp: &Exp, unop: asm::Unop) -> Value<Temp> {
        match self.tile_exp(exp) {
        | Value::Mem(mem) => {
            self.asm.push(asm::Asm::Un(unop, asm::Unary::M(mem)));
            Value::Mem(mem)
        },
        | temp => {
            let temp = self.into_temp(temp);
            self.asm.push(asm::Asm::Un(unop, asm::Unary::R(temp)));
            Value::Reg(temp)
        },
        }
    }
}
