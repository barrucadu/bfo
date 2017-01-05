use std::env;
use std::fs::File;
use std::io::Read;
use std::path::Path;

fn main() {
    if let Some(fname) = env::args().nth(1) {
        if let Ok(mut file) = File::open(Path::new(&fname)) {
            let mut code = String::new();
            if file.read_to_string(&mut code).is_ok() {
                if let Some(compiled) = compile(code) {
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
    ntimes: u8,
    index: usize,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Op {
    Add,
    Sub,
    Left,
    Right,
    PutCh,
    GetCh,
    JZ,
    JNZ,
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

fn compile(code: String) -> Option<Vec<Instr>> {
    let mut instrs = Vec::new();
    let mut jumps = Vec::new();
    let mut accumulating = None;
    let mut accumulated: u8 = 0;
    for c in code.chars() {
        if let Some(op) = opcode(c) {
            // If we've squashed some opcodes, and now are changing
            // the operation, store the squahed one.
            if let Some(acc_op) = accumulating {
                if acc_op != op || accumulated == 255 {
                    instrs.push(Instr {
                        opcode: acc_op,
                        ntimes: accumulated,
                        index: 0,
                    });
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
                        ntimes: 0,
                        index: 0,
                    });
                    jumps.push(instrs.len() - 1);
                }

                // For a loop closer, compile it to a JNZ with a
                // target of the opener, and update the target of the
                // opener.
                Op::JNZ => {
                    match jumps.pop() {
                        Some(start) => {
                            instrs.push(Instr {
                                opcode: Op::JNZ,
                                ntimes: 0,
                                index: start,
                            });
                            instrs[start].index = instrs.len() - 1;
                        }
                        None => return None,
                    }
                }
            }
        }
    }
    if jumps.len() == 0 { Some(instrs) } else { None }
}

fn run(code: Vec<Instr>) {
    let mut ip = 0;
    let mut memory: [u8; 30000] = [0; 30000];
    let mut dp = 0;

    while ip < code.len() {
        let instr = code[ip];
        match instr.opcode {
            Op::Add => {
                memory[dp] = memory[dp].wrapping_add(instr.ntimes);
            }
            Op::Sub => {
                memory[dp] = memory[dp].wrapping_sub(instr.ntimes);
            }
            Op::Left => {
                dp = dp.saturating_sub(instr.ntimes as usize);
            }
            Op::Right => {
                dp = dp.saturating_add(instr.ntimes as usize);
            }
            Op::PutCh => {
                for _ in 0..instr.ntimes {
                    print!("{}", memory[dp] as char);
                }
            }
            Op::GetCh => {
                // Only the last character input will be kept, but
                // only asking for one character would change the
                // program semantics.
                for _ in 0..instr.ntimes {
                    let inp: Option<u8> = std::io::stdin()
                        .bytes()
                        .next()
                        .and_then(|result| result.ok());
                    if let Some(inp_u8) = inp {
                        memory[dp] = inp_u8;
                    }
                }
            }
            Op::JZ => {
                if memory[dp] == 0 {
                    ip = instr.index
                };
            }
            Op::JNZ => {
                if memory[dp] != 0 {
                    ip = instr.index
                };
            }
        }
        ip += 1;
    }
}
