use std::env;
use std::fs::File;
use std::io::Read;
use std::path::Path;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Options {
    // Fuse adjacent equal operations.
    fuse_adjacent: bool,
    // Fuse set/{add,sub} pairs.
    fuse_set_add: bool,
    // Compress set-zero loops into a single instruction.
    loop_set_zero: bool,
    // Compress copy/multiply loops into a sequence of copy/multiply
    // instructions.
    loop_copy_multiply: bool,
    // Compress left/right-seek loops into a single instruction.
    loop_seek_lr: bool,
    // Use a set before the sart or end of a loop to change the
    // condition.
    loop_set_jump: bool,
}

fn main() {
    let opts = Options {
        fuse_adjacent: true,
        fuse_set_add: true,
        loop_set_zero: true,
        loop_copy_multiply: true,
        loop_seek_lr: false,
        loop_set_jump: true,
    };


    if let Some(fname) = env::args().nth(1) {
        if let Ok(mut file) = File::open(Path::new(&fname)) {
            let mut code = String::new();
            if file.read_to_string(&mut code).is_ok() {
                if let Some(compiled) = compile(code, opts) {
                    run(compiled);
                } else {
                    println!("ERROR: could not compile code (are your brackets matched?");
                }
            }
        } else {
            println!("ERROR: could not open file.");
        }
    } else {
        println!("USAGE: bfo <file>");
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Instr {
    opcode: Op,
    arg: u8,
    off: i32,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Op {
    Add,
    Sub,
    Left,
    Right,
    PutCh,
    GetCh,
    J,
    JZ,
    JNZ,
    Set,
    CMul,
    CNMul,
    SeekL,
    SeekR,
}

fn opcode(c: char) -> Option<Op> {
    match c {
        '+' => Some(Op::Add),
        '-' => Some(Op::Sub),
        '<' => Some(Op::Left),
        '>' => Some(Op::Right),
        '.' => Some(Op::PutCh),
        ',' => Some(Op::GetCh),
        '[' => Some(Op::JZ),
        ']' => Some(Op::JNZ),
        _ => None,
    }
}

fn compile(code: String, opts: Options) -> Option<Vec<Instr>> {
    let mut instrs = Vec::new();
    let mut jumps = Vec::new();
    let mut accumulating = None;
    let mut accumulated: u8 = 0;
    for c in code.chars() {
        if let Some(op) = opcode(c) {
            // If we've squashed some opcodes, and now are changing
            // the operation, store the squashed one.
            if let Some(acc_op) = accumulating {
                if acc_op != op || accumulated == 255 || !opts.fuse_adjacent {
                    let acc_instr = Instr {
                        opcode: acc_op,
                        arg: accumulated,
                        off: 0,
                    };
                    if instrs.len() > 0 && opts.fuse_set_add {
                        let prior_idx = instrs.len() - 1;
                        let prior: Instr = instrs[prior_idx];
                        match (prior.opcode, acc_op) {
                            (Op::Set, Op::Add) => {
                                instrs[prior_idx].arg = prior.arg.wrapping_add(accumulated)
                            }
                            (Op::Set, Op::Sub) => {
                                instrs[prior_idx].arg = prior.arg.wrapping_sub(accumulated)
                            }
                            _ => instrs.push(acc_instr),
                        };
                    } else {
                        instrs.push(acc_instr);
                    }
                    accumulating = None;
                    accumulated = 0;
                }
            }

            match op {
                // For the non-loop opcodes, squash adjacent ones.
                Op::Add | Op::Sub | Op::Left | Op::Right | Op::PutCh | Op::GetCh => {
                    accumulating = Some(op);
                    accumulated += 1;
                }

                // For a loop opener, compile it to a JZ with initial
                // target of 0, then push the current position for use
                // when the closer is found.
                Op::JZ => {
                    instrs.push(Instr {
                        opcode: Op::JZ,
                        arg: 0,
                        off: 0,
                    });
                    jumps.push(instrs.len() - 1);
                }

                // For a loop closer, compile it to a JNZ with a
                // target of the opener, and update the target of the
                // opener.
                Op::JNZ => {
                    match jumps.pop() {
                        Some(start) => {
                            let off = start as i32 - instrs.len() as i32;
                            instrs.push(Instr {
                                opcode: Op::JNZ,
                                arg: 0,
                                off: off,
                            });
                            instrs[start].off = -off;
                            if let Some(mut optimised) = optimise_loop(&instrs, start, opts) {
                                instrs.truncate(start);
                                instrs.append(&mut optimised);
                            }
                        }
                        None => return None,
                    }
                }

                // The other opcodes only show up as a result of
                // optimisation, they shouldn't be returned by the
                // opcode function.
                _ => panic!("Unexpected opcode: {:?}", op),
            }
        }
    }
    if let Some(acc_op) = accumulating {
        instrs.push(Instr {
            opcode: acc_op,
            arg: accumulated,
            off: 0,
        });
    }
    if jumps.len() == 0 { Some(instrs) } else { None }
}

fn optimise_loop(code: &Vec<Instr>, start: usize, opts: Options) -> Option<Vec<Instr>> {
    // If a loop only touches one cell and (overall) increases or
    // decreases the value, then it zeroes the cell.
    //
    // Examples: [-], [+], [--+]
    let set_zero = || {
        if !opts.loop_set_zero {
            return None;
        }

        let mut delta: i32 = 0;
        for i in start + 1..code.len() - 1 {
            match code[i].opcode {
                Op::Add => delta += 1,
                Op::Sub => delta -= 1,
                _ => return None,
            }
        }
        if delta != 0 {
            Some(vec![Instr {
                          opcode: Op::Set,
                          arg: 0,
                          off: 0,
                      }])
        } else {
            None
        }
    };

    // If a loop is of the form [->+<], it copies a value from one
    // cell to the next. Multiple cells could be copied in to. The
    // copying may also have a multiplicative factor.
    let copy_multiply = || {
        if !opts.loop_copy_multiply {
            return None;
        }

        if code.len() <= start + 2 {
            return None;
        }

        let mut fst_del: i32 = 0;
        let mut deltas: Vec<(i32, i32)> = Vec::new();
        let mut off: i32 = 0;
        for i in start + 1..code.len() - 1 {
            match code[i].opcode {
                // Moving right and left changes the cell offset.
                Op::Right => off += code[i].arg as i32,
                Op::Left if off >= code[i].arg as i32 => off -= code[i].arg as i32,
                // Adding to a nn-first cell adds a multiplicative factor.
                Op::Add if off != 0 => deltas.push((code[i].arg as i32, off)),
                Op::Sub if off != 0 => deltas.push((-(code[i].arg as i32), off)),
                // Adding or subtracting from the initial cell updates its delta.
                Op::Add => fst_del += code[i].arg as i32,
                Op::Sub => fst_del -= code[i].arg as i32,
                _ => return None,
            }
        }

        // Final offset must be 0, or we haven't returned to the initial cell.
        if off != 0 {
            return None;
        }

        // Final delta of the original cell must be -1.
        if fst_del != -1 {
            return None;
        }

        let mut instrs = Vec::new();
        for (del, off) in deltas {
            instrs.push(Instr {
                opcode: if del < 0 { Op::CNMul } else { Op::CMul },
                arg: if del < 0 { -del } else { del } as u8,
                off: off,
            });
        }
        instrs.push(Instr {
            opcode: Op::Set,
            arg: 0,
            off: 0,
        });
        Some(instrs)
    };

    // Replace [<] and [>] with a single "seek left" or "seek right"
    // operation.
    let seek_lr = || {
        if !opts.loop_seek_lr {
            return None;
        }

        if code.len() != start + 3 {
            return None;
        }

        match code[start + 1].opcode {
            Op::Left if code[start + 1].arg == 1 => {
                Some(vec![Instr {
                              opcode: Op::SeekL,
                              arg: 0,
                              off: 0,
                          }])
            }
            Op::Right if code[start + 1].arg == 1 => {
                Some(vec![Instr {
                              opcode: Op::SeekR,
                              arg: 0,
                              off: 0,
                          }])
            }
            _ => None,
        }
    };

    // Turn a set followed by a conditional jump into a set followed
    // by an unconditional jump
    let set_jump = || {
        if !opts.loop_set_jump {
            return None;
        }

        let before1 = code[start - 1];
        let before2 = code[code.len() - 2];
        if start > 0 && before1.opcode == Op::Set && before1.arg == 0 {
            // Loop opener is JZ, so Set 0; [ ... ] ==> Set 0.
            Some(vec![])
        } else if before2.opcode == Op::Set {
            // Copy the loop body, as we're changing the end.
            let mut instrs = Vec::new();
            for i in start..code.len() - 2 {
                instrs.push(code[i]);
            }

            // Loop closer jump can be made unconditional if
            // immediately preceeded by a Set.
            if before2.arg == 0 {
                // If it's a set 0, omit the jump, as we leave the
                // loop here (an unconditional jump to the same place
                // would also work, but be one extra instruction).
                instrs[0].off -= 2;
            } else {
                // If it's a set !0, unconditional jump to the start
                // of the loop.
                instrs.push(Instr {
                    opcode: Op::J,
                    arg: 0,
                    off: code[code.len() - 1].off,
                });
            }

            Some(instrs)
        } else {
            None
        }
    };

    set_zero().or(copy_multiply().or(seek_lr().or(set_jump())))
}

fn run(code: Vec<Instr>) {
    let mut ip = 0;
    let mut memory: [u8; 30000] = [0; 30000];
    let mut dp = 0;

    while ip < code.len() {
        let instr = code[ip];
        match instr.opcode {
            Op::Add => {
                memory[dp] = memory[dp].wrapping_add(instr.arg);
            }
            Op::Sub => {
                memory[dp] = memory[dp].wrapping_sub(instr.arg);
            }
            Op::Left => {
                dp = dp.saturating_sub(instr.arg as usize);
            }
            Op::Right => {
                dp = dp.saturating_add(instr.arg as usize);
            }
            Op::PutCh => {
                for _ in 0..instr.arg {
                    print!("{}", memory[dp] as char);
                }
            }
            Op::GetCh => {
                // Only the last character input will be kept, but
                // only asking for one character would change the
                // program semantics.
                for _ in 0..instr.arg {
                    let inp: Option<u8> = std::io::stdin()
                        .bytes()
                        .next()
                        .and_then(|result| result.ok());
                    if let Some(inp_u8) = inp {
                        memory[dp] = inp_u8;
                    }
                }
            }
            Op::J => ip = (ip as i32 + instr.off) as usize,
            Op::JZ => {
                if memory[dp] == 0 {
                    ip = (ip as i32 + instr.off) as usize
                }
            }
            Op::JNZ => {
                if memory[dp] != 0 {
                    ip = (ip as i32 + instr.off) as usize
                }
            }
            Op::Set => {
                memory[dp] = instr.arg;
            }
            Op::CMul => {
                let tgt = (dp as i32 + instr.off) as usize;
                memory[tgt] = memory[tgt].wrapping_add(memory[dp].wrapping_mul(instr.arg));
            }
            Op::CNMul => {
                let tgt = (dp as i32 + instr.off) as usize;
                memory[tgt] = memory[tgt].wrapping_sub(memory[dp].wrapping_mul(instr.arg));
            }
            Op::SeekL => {
                while memory[dp] > 0 {
                    dp -= 1;
                }
            }
            Op::SeekR => {
                while memory[dp] > 0 {
                    dp += 1;
                }
            }
        }
        ip += 1;
    }
}
