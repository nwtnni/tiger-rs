use codespan::ByteSpan;
use im::HashMap;
use uuid::Uuid;

use ast::*;
use error::{Error, TypeError};

#[derive(PartialEq, Eq, Clone)]
pub enum Ty {
    Nil,
    Int,
    Str,
    Unit,
    Arr(Box<Ty>, Uuid),
    Rec(Vec<(String, Ty)>, Uuid),
    Name(String, Option<Box<Ty>>),
}

impl Ty {

    pub fn is_arr(&self) -> bool {
        match self {
        | Ty::Arr(_, _) => true,
        | _             => false,
        }
    }

    pub fn is_rec(&self) -> bool {
        match self {
        | Ty::Rec(_, _) => true,
        | _             => false,
        }
    }
}

#[derive(PartialEq, Eq)]
pub struct Typed {
    ty: Ty,
    _exp: (),
}

pub enum Binding {
    Var(Ty, bool),
    Fun(Vec<Ty>, Ty),
}

type Context<T> = HashMap<String, T>;
type TypeContext = Context<Ty>;
type VarContext = Context<Binding>;

fn ok(ty: Ty) -> Result<Typed, Error> {
    Ok(Typed { ty, _exp: () })
}

fn error<T>(span: &ByteSpan, err: TypeError) -> Result<T, Error> {
    Err(Error::semantic(*span, err))
}

pub struct Checker {
    loops: Vec<()>,
}

impl Checker {

    pub fn check(ast: &Exp) -> Result<(), Error> {

        let vc = hashmap! {
            "print".to_string()     => Binding::Fun(vec![Ty::Str], Ty::Unit),
            "flush".to_string()     => Binding::Fun(vec![], Ty::Unit),
            "getchar".to_string()   => Binding::Fun(vec![], Ty::Str),
            "ord".to_string()       => Binding::Fun(vec![Ty::Str], Ty::Int),
            "chr".to_string()       => Binding::Fun(vec![Ty::Int], Ty::Str),
            "size".to_string()      => Binding::Fun(vec![Ty::Str], Ty::Int),
            "substring".to_string() => Binding::Fun(vec![Ty::Str, Ty::Int, Ty::Int], Ty::Str),
            "concat".to_string()    => Binding::Fun(vec![Ty::Str, Ty::Str], Ty::Str),
            "not".to_string()       => Binding::Fun(vec![Ty::Int], Ty::Int),
            "exit".to_string()      => Binding::Fun(vec![Ty::Int], Ty::Unit)
        };

        let tc = hashmap! {
            "int".to_string()    => Ty::Int,
            "string".to_string() => Ty::Str
        };

        let mut checker = Checker { loops: Vec::new() };
        let _ = checker.check_exp(vc, tc, ast)?;
        Ok(())
    }

    fn check_var(&mut self, vc: VarContext, tc: TypeContext, var: &Var) -> Result<Typed, Error> {

        macro_rules! is_int {
            ($exp:expr) => { self.check_exp(vc.clone(), tc.clone(), $exp)?.ty == Ty::Int }
        }

        match var {
        | Var::Simple(name, span) => {

            // Unbound in type context
            if !tc.contains_key(name) {
                return error(span, TypeError::UnboundType)
            }

            ok(tc[name].clone())
        },
        | Var::Field(rec, field, span) => {

            // Must be bound to record type
            match self.check_var(vc, tc, &*rec)?.ty {
            | Ty::Rec(fields, _) => {

                // Find corresponding field
                let ty = fields.iter()
                    .find(|(name, _)| field == name)
                    .map(|(_, ty)| ty);

                match ty {
                | Some(ty) => ok(ty.clone()),
                | None     => error(span, TypeError::UnboundField),
                }
            },
            | _ => error(span, TypeError::NotRecord),
            }
        },
        | Var::Index(arr, index, span) => {

            // Index must be integer
            if !is_int!(&*index) {
                return error(span, TypeError::IndexMismatch)
            }

            // Get element type
            match self.check_var(vc, tc, &*arr)?.ty {
            | Ty::Arr(elem, _) => ok(*elem.clone()),
            | _                => error(span, TypeError::NotArr),
            }
        },
        }
    }

    fn check_exp(&mut self, vc: VarContext, tc: TypeContext, exp: &Exp) -> Result<Typed, Error> {

        macro_rules! is_int {
            ($exp:expr) => { self.check_exp(vc.clone(), tc.clone(), $exp)?.ty == Ty::Int }
        }

        macro_rules! is_unit {
            ($exp:expr) => { self.check_exp(vc.clone(), tc.clone(), $exp)?.ty == Ty::Unit }
        }

        match exp {
        | Exp::Break(span) => {

            if self.loops.is_empty() {
                return error(span, TypeError::Break)
            }

            ok(Ty::Unit)

        },
        | Exp::Nil(_)                  => ok(Ty::Nil),
        | Exp::Int(_, _)               => ok(Ty::Int),
        | Exp::Str(_, _)               => ok(Ty::Str),
        | Exp::Var(var, _)             => self.check_var(vc, tc, var),
        | Exp::Call{name, args, span} => {

            if !vc.contains_key(name) { return error(span, TypeError::UnboundFunction) }

            match &vc[name] {
            | Binding::Var(_, _) => error(span, TypeError::NotFunction),
            | Binding::Fun(args_ty, ret_ty) => {

                if args.len() != args_ty.len() {
                    return error(span, TypeError::CallMismatch)
                }

                for (arg, ty) in args.iter().zip(args_ty) {
                    if &self.check_exp(vc.clone(), tc.clone(), arg)?.ty != ty {
                        return error(span, TypeError::CallMismatch)
                    }
                }

                ok(ret_ty.clone())
            },
            }
        },
        | Exp::Neg(exp, span) => {

            if !is_int!(&*exp) { return error(span, TypeError::Neg) }

            ok(Ty::Int)

        },
        | Exp::Bin{lhs, op, rhs, span} => {

            let lt = self.check_exp(vc.clone(), tc.clone(), lhs)?.ty;
            let rt = self.check_exp(vc, tc, rhs)?.ty;

            // No binary operators work on unit
            if lt == Ty::Unit || rt == Ty::Unit {
                return error(span, TypeError::BinaryMismatch)
            }

            // Equality checking is valid for:
            // - Rec and Nil
            // - Nil and Rec
            // - Rec and Rec
            // - Nil and Nil
            // - Str and Str
            // - Int and Int
            // - Arr and Arr
            if op.is_equality() && (lt == rt || lt.is_rec() && rt == Ty::Nil || lt == Ty::Nil && rt.is_rec()) {
                return ok(Ty::Int)
            }

            // Comparisons are valid for
            // - Str and Str
            // - Int and Int
            if op.is_comparison() && (lt == Ty::Int || lt == Ty::Str) && lt == rt {
                return ok(Ty::Int)
            }

            // Arithmetic is valid for
            // - Int and Int
            if lt == Ty::Int && rt == Ty::Int {
                return ok(Ty::Int)
            }

            error(span, TypeError::BinaryMismatch)
        },
        | Exp::Rec{name,fields,span} => {

            if !tc.contains_key(name) {
                return error(span, TypeError::UnboundRecord)
            }

            match &tc[name] {
            | Ty::Rec(fields_ty, _) => {

                if fields.len() != fields_ty.len() {
                    return error(span, TypeError::FieldMismatch)
                }

                // Check all field name - value pairs
                for (field, (field_name, field_ty)) in fields.iter().zip(fields_ty) {
                    if &field.name != field_name || &self.check_exp(vc.clone(), tc.clone(), &*field.exp)?.ty != field_ty {
                        return error(span, TypeError::FieldMismatch)
                    }
                }

                ok((&tc[name]).clone())
            },
            | _ => error(span, TypeError::NotRecord),
            }
        },
        | Exp::Seq(exps, span) => {

            // Empty sequence is just unit
            if exps.len() == 0 {
                return ok(Ty::Unit)
            }

            // Make sure all intermediate steps return unit
            if exps.len() > 1 {
                for i in 0..exps.len() - 1 {
                    if !is_unit!(&exps[i]) { return error(span, TypeError::UnusedExp) }
                }
            }

            // Result is type of last exp
            self.check_exp(vc, tc, &exps.last().unwrap())
        },
        | Exp::Ass{name, exp, span} => {

            let var = self.check_var(vc.clone(), tc.clone(), name)?.ty;

            if self.check_exp(vc, tc, exp)?.ty != var {
                return error(span, TypeError::VarMismatch)
            }

            ok(Ty::Unit)
        },
        | Exp::If{guard, then, or, span} => {

            // Guard must be boolean
            if !is_int!(&*guard) {
                return error(span, TypeError::GuardMismatch)
            }

            // Check type of if branch
            let then_ty = self.check_exp(vc.clone(), tc.clone(), &*then)?.ty;

            if let Some(exp) = or {

                // For if-else, both branches must return the same type
                if self.check_exp(vc, tc, &*exp)?.ty != then_ty {
                    return error(span, TypeError::BranchMismatch)
                }

                ok(then_ty.clone())

            } else {

                // For if, branch must have no expression
                if then_ty != Ty::Unit {
                    return error(span, TypeError::UnusedBranch)
                }

                ok(Ty::Unit)
            }
        },
        | Exp::While{guard, body, span} => {

            // Guard must be boolean
            if !is_int!(&*guard) {
                return error(span, TypeError::GuardMismatch)
            }

            // Enter loop body
            self.loops.push(());

            // Body must be unit
            if !is_unit!(&*body) {
                return error(span, TypeError::UnusedWhileBody)
            }

            ok(Ty::Unit)
        },
        | Exp::For{name, lo, hi, body, span, ..} => {

            if !is_int!(&*lo) {
                return error(span, TypeError::ForBound)
            }

            if !is_int!(&*hi) {
                return error(span, TypeError::ForBound)
            }

            // Bind loop variable as immutable
            let for_vc = vc.insert(name.clone(), Binding::Var(Ty::Int, false));

            // Enter loop body
            self.loops.push(());

            // Check body with updated VarContext
            if self.check_exp(for_vc, tc, &*body)?.ty != Ty::Unit {
                return error(span, TypeError::UnusedForBody)
            }

            ok(Ty::Unit)
        },
        | Exp::Let{decs, body, ..} => {

            let (mut let_vc, mut let_tc) = (vc.clone(), tc.clone());

            for dec in decs {
                let (new_vc, new_tc) = self.check_dec(let_vc, let_tc, &*dec)?;
                let_vc = new_vc;
                let_tc = new_tc;
            }

            self.check_exp(let_vc, let_tc, &*body)
        },
        | Exp::Arr{name, size, init, span} => {

            if !tc.contains_key(name) {
                return error(span, TypeError::UnboundArr)
            }

            let elem = match &tc[name] {
            | Ty::Arr(elem, _) => &**elem,
            | _                => return error(span, TypeError::NotArr),
            };

            if !is_int!(&*size) {
                return error(span, TypeError::ForBound)
            }

            if &self.check_exp(vc.clone(), tc.clone(), &*init)?.ty != elem {
                return error(span, TypeError::ArrMismatch)
            }

            ok((&tc[name]).clone())
        },
        }
    }

    fn check_dec(&self, vc: VarContext, tc: TypeContext, dec: &Dec) -> Result<(VarContext, TypeContext), Error> {

        unreachable!()
    }

    fn check_type(&self, tc: TypeContext, ty: &Type) -> Result<Ty, Error> {

        unreachable!()
    }

}
