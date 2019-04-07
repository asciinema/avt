// This parser is based on Paul Williams' parser for ANSI-compatible video
// terminals: https://www.vt100.net/emu/dec_ansi_parser

#[derive(Debug)]
enum State {
    Ground,
    Escape,
    EscapeIntermediate,
    CsiEntry,
    CsiParam,
    CsiIntermediate,
    CsiIgnore,
    DcsEntry,
    DcsParam,
    DcsIntermediate,
    DcsPassthrough,
    DcsIgnore,
    OscString,
    SosPmApcString,
}

#[derive(Debug)]
pub struct VT {
    state: State,
}

impl VT {
    pub fn new() -> Self {
        VT {
            state: State::Ground,
        }
    }

    pub fn feed(&mut self, input: char) {
        let input2 = if input >= '\u{a0}' { '\u{41}' } else { input };

        match (&self.state, input2) {
            (_, '\u{a0}'...'\u{10ffff}') => {}

            // Anywhere
            (_, '\u{18}')
            | (_, '\u{1a}')
            | (_, '\u{80}'...'\u{8f}')
            | (_, '\u{91}'...'\u{97}')
            | (_, '\u{99}')
            | (_, '\u{9a}') => {
                self.state = State::Ground;
                self.execute(input);
            }

            (_, '\u{1b}') => {
                self.state = State::Escape;
            }

            (_, '\u{90}') => {
                self.state = State::DcsEntry;
            }

            (_, '\u{9b}') => {
                self.state = State::CsiEntry;
            }

            (_, '\u{9c}') => {
                self.state = State::Ground;
            }

            (_, '\u{9d}') => {
                self.state = State::OscString;
            }

            (_, '\u{98}') | (_, '\u{9e}') | (_, '\u{9f}') => {
                self.state = State::SosPmApcString;
            }

            // Ground

            // C0 prime
            (State::Ground, '\u{00}'...'\u{17}')
            | (State::Ground, '\u{19}')
            | (State::Ground, '\u{1c}'...'\u{1f}') => {
                self.execute(input);
            }

            (State::Ground, '\u{20}'...'\u{7f}') => {
                // (State::Ground, '\u{a0}'...'\u{ff}') => {
                self.print(input);
            }

            // Escape

            // C0 prime
            (State::Escape, '\u{00}'...'\u{17}')
            | (State::Escape, '\u{19}')
            | (State::Escape, '\u{1c}'...'\u{1f}') => {
                self.execute(input);
            }

            (State::Escape, '\u{20}'...'\u{2f}') => {
                self.state = State::EscapeIntermediate;
                self.collect(input);
            }

            (State::Escape, '\u{30}'...'\u{4f}')
            | (State::Escape, '\u{51}'...'\u{57}')
            | (State::Escape, '\u{59}')
            | (State::Escape, '\u{5a}')
            | (State::Escape, '\u{5c}')
            | (State::Escape, '\u{60}'...'\u{7e}') => {
                self.state = State::Ground;
                self.esc_dispatch(input);
            }

            (State::Escape, '\u{50}') => {
                self.state = State::DcsEntry;
            }

            (State::Escape, '\u{5b}') => {
                self.state = State::CsiEntry;
            }

            (State::Escape, '\u{5d}') => {
                self.state = State::OscString;
            }

            (State::Escape, '\u{58}') | (State::Escape, '\u{5e}') | (State::Escape, '\u{5f}') => {
                self.state = State::SosPmApcString;
            }

            (State::Escape, '\u{7f}') => {
                self.ignore(input);
            }

            // EscapeIntermediate

            // C0 prime
            (State::EscapeIntermediate, '\u{00}'...'\u{17}')
            | (State::EscapeIntermediate, '\u{19}')
            | (State::EscapeIntermediate, '\u{1c}'...'\u{1f}') => {
                self.execute(input);
            }

            (State::EscapeIntermediate, '\u{20}'...'\u{2f}') => {
                self.collect(input);
            }

            (State::EscapeIntermediate, '\u{30}'...'\u{7e}') => {
                self.state = State::Ground;
                self.esc_dispatch(input);
            }

            (State::EscapeIntermediate, '\u{7f}') => {
                self.ignore(input);
            }

            // CsiEntry

            // C0 prime
            (State::CsiEntry, '\u{00}'...'\u{17}')
            | (State::CsiEntry, '\u{19}')
            | (State::CsiEntry, '\u{1c}'...'\u{1f}') => {
                self.execute(input);
            }

            (State::CsiEntry, '\u{20}'...'\u{2f}') => {
                self.state = State::CsiIntermediate;
                self.collect(input);
            }

            (State::CsiEntry, '\u{30}'...'\u{39}') | (State::CsiEntry, '\u{3b}') => {
                self.state = State::CsiParam;
                self.param(input);
            }

            (State::CsiEntry, '\u{3a}') => {
                self.state = State::CsiIgnore;
            }

            (State::CsiEntry, '\u{3c}'...'\u{3f}') => {
                self.state = State::CsiParam;
                self.collect(input);
            }

            (State::CsiEntry, '\u{40}'...'\u{7e}') => {
                self.state = State::Ground;
                self.csi_dispatch(input);
            }

            (State::CsiEntry, '\u{7f}') => {
                self.ignore(input);
            }

            // CsiParam

            // C0 prime
            (State::CsiParam, '\u{00}'...'\u{17}')
            | (State::CsiParam, '\u{19}')
            | (State::CsiParam, '\u{1c}'...'\u{1f}') => {
                self.execute(input);
            }

            (State::CsiParam, '\u{20}'...'\u{2f}') => {
                self.state = State::CsiIntermediate;
                self.collect(input);
            }

            (State::CsiParam, '\u{30}'...'\u{39}') | (State::CsiParam, '\u{3b}') => {
                self.param(input);
            }

            (State::CsiParam, '\u{3a}') | (State::CsiParam, '\u{3c}'...'\u{3f}') => {
                self.state = State::CsiIgnore;
            }

            (State::CsiParam, '\u{40}'...'\u{7e}') => {
                self.state = State::Ground;
                self.csi_dispatch(input);
            }

            (State::CsiParam, '\u{7f}') => {
                self.ignore(input);
            }

            // CsiIntermediate

            // C0 prime
            (State::CsiIntermediate, '\u{00}'...'\u{17}')
            | (State::CsiIntermediate, '\u{19}')
            | (State::CsiIntermediate, '\u{1c}'...'\u{1f}') => {
                self.execute(input);
            }

            (State::CsiIntermediate, '\u{20}'...'\u{2f}') => {
                self.collect(input);
            }

            (State::CsiIntermediate, '\u{30}'...'\u{3f}') => {
                self.state = State::CsiIgnore;
            }

            (State::CsiIntermediate, '\u{40}'...'\u{7e}') => {
                self.state = State::Ground;
                self.csi_dispatch(input);
            }

            (State::CsiIntermediate, '\u{7f}') => {
                self.ignore(input);
            }

            // CsiIgnore

            // C0 prime
            (State::CsiIgnore, '\u{00}'...'\u{17}')
            | (State::CsiIgnore, '\u{19}')
            | (State::CsiIgnore, '\u{1c}'...'\u{1f}') => {
                self.execute(input);
            }

            (State::CsiIgnore, '\u{20}'...'\u{3f}') => {
                self.ignore(input);
            }

            (State::CsiIgnore, '\u{40}'...'\u{7e}') => {
                self.state = State::Ground;
            }

            (State::CsiIgnore, '\u{7f}') => {
                self.ignore(input);
            }

            // DcsEntry

            // C0 prime
            (State::DcsEntry, '\u{00}'...'\u{17}')
            | (State::DcsEntry, '\u{19}')
            | (State::DcsEntry, '\u{1c}'...'\u{1f}') => {
                self.ignore(input);
            }

            (State::DcsEntry, '\u{20}'...'\u{2f}') => {
                self.state = State::DcsIntermediate;
                self.collect(input);
            }

            (State::DcsEntry, '\u{30}'...'\u{39}') | (State::DcsEntry, '\u{3b}') => {
                self.state = State::DcsParam;
                self.param(input);
            }

            (State::DcsEntry, '\u{3c}'...'\u{3f}') => {
                self.state = State::DcsParam;
                self.collect(input);
            }

            (State::DcsEntry, '\u{3a}') => {
                self.state = State::DcsIgnore;
            }

            (State::DcsEntry, '\u{40}'...'\u{7e}') => {
                self.state = State::DcsPassthrough;
            }

            (State::DcsEntry, '\u{7f}') => {
                self.ignore(input);
            }

            // DcsParam

            // C0 prime
            (State::DcsParam, '\u{00}'...'\u{17}')
            | (State::DcsParam, '\u{19}')
            | (State::DcsParam, '\u{1c}'...'\u{1f}') => {
                self.ignore(input);
            }

            (State::DcsParam, '\u{20}'...'\u{2f}') => {
                self.state = State::DcsIntermediate;
                self.collect(input);
            }

            (State::DcsParam, '\u{30}'...'\u{39}') | (State::DcsParam, '\u{3b}') => {
                self.param(input);
            }

            (State::DcsParam, '\u{3a}') | (State::DcsParam, '\u{3c}'...'\u{3f}') => {
                self.state = State::DcsIgnore;
            }

            (State::DcsParam, '\u{40}'...'\u{7e}') => {
                self.state = State::DcsPassthrough;
            }

            (State::DcsParam, '\u{7f}') => {
                self.ignore(input);
            }

            // DcsIntermediate

            // C0 prime
            (State::DcsIntermediate, '\u{00}'...'\u{17}')
            | (State::DcsIntermediate, '\u{19}')
            | (State::DcsIntermediate, '\u{1c}'...'\u{1f}') => {
                self.ignore(input);
            }

            (State::DcsIntermediate, '\u{20}'...'\u{2f}') => {
                self.collect(input);
            }

            (State::DcsIntermediate, '\u{30}'...'\u{3f}') => {
                self.state = State::DcsIgnore;
            }

            (State::DcsIntermediate, '\u{40}'...'\u{7e}') => {
                self.state = State::DcsPassthrough;
            }

            (State::DcsIntermediate, '\u{7f}') => {
                self.ignore(input);
            }

            // DcsPassthrough

            // C0 prime
            (State::DcsPassthrough, '\u{00}'...'\u{17}')
            | (State::DcsPassthrough, '\u{19}')
            | (State::DcsPassthrough, '\u{1c}'...'\u{1f}') => {
                self.put(input);
            }

            (State::DcsPassthrough, '\u{20}'...'\u{7e}') => {
                self.put(input);
            }

            (State::DcsPassthrough, '\u{7f}') => {
                self.ignore(input);
            }

            // DcsIgnore

            // C0 prime
            (State::DcsIgnore, '\u{00}'...'\u{17}')
            | (State::DcsIgnore, '\u{19}')
            | (State::DcsIgnore, '\u{1c}'...'\u{1f}') => {
                self.ignore(input);
            }

            (State::DcsIgnore, '\u{20}'...'\u{7f}') => {
                self.ignore(input);
            }

            // OscString

            // C0 prime (without 0x07)
            (State::OscString, '\u{00}'...'\u{06}')
            | (State::OscString, '\u{08}'...'\u{17}')
            | (State::OscString, '\u{19}')
            | (State::OscString, '\u{1c}'...'\u{1f}') => {
                self.ignore(input);
            }

            (State::OscString, '\u{07}') => {
                // 0x07 is xterm non-ANSI variant of transition to ground
                self.state = State::Ground;
            }

            (State::OscString, '\u{20}'...'\u{7f}') => {
                self.osc_put(input);
            }

            // SosPmApcString

            // C0 prime
            (State::SosPmApcString, '\u{00}'...'\u{17}')
            | (State::SosPmApcString, '\u{19}')
            | (State::SosPmApcString, '\u{1c}'...'\u{1f}') => {
                self.ignore(input);
            }

            (State::SosPmApcString, '\u{20}'...'\u{7f}') => {
                self.ignore(input);
            }
        }
    }

    fn execute(&self, input: char) {
        print!("execute\n");
    }

    fn print(&self, input: char) {
        print!("print\n");
    }

    fn collect(&self, input: char) {
        print!("collect\n");
    }

    fn esc_dispatch(&self, input: char) {
        print!("esc_dispatch\n");
    }

    fn ignore(&self, input: char) {
        print!("ignore\n");
    }

    fn param(&self, input: char) {
        print!("param\n");
    }

    fn csi_dispatch(&self, input: char) {
        print!("csi_dispatch\n");
    }

    fn put(&self, input: char) {
        print!("put\n");
    }

    fn osc_put(&self, input: char) {
        print!("osc_put\n");
    }
}
