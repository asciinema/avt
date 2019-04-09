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
    params: Vec<char>,
    intermediates: Vec<char>,
}

impl VT {
    pub fn new() -> Self {
        VT {
            state: State::Ground,
            params: Vec::new(),
            intermediates: Vec::new(),
        }
    }

    pub fn feed(&mut self, input: char) {
        let input2 = if input >= '\u{a0}' { '\u{41}' } else { input };

        match (&self.state, input2) {
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
                self.clear();
            }

            (_, '\u{90}') => {
                self.state = State::DcsEntry;
                self.clear();
            }

            (_, '\u{9b}') => {
                self.state = State::CsiEntry;
                self.clear();
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
                self.clear();
            }

            (State::Escape, '\u{5b}') => {
                self.state = State::CsiEntry;
                self.clear();
            }

            (State::Escape, '\u{5d}') => {
                self.state = State::OscString;
            }

            (State::Escape, '\u{58}') | (State::Escape, '\u{5e}') | (State::Escape, '\u{5f}') => {
                self.state = State::SosPmApcString;
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

            // CsiIgnore

            // C0 prime
            (State::CsiIgnore, '\u{00}'...'\u{17}')
            | (State::CsiIgnore, '\u{19}')
            | (State::CsiIgnore, '\u{1c}'...'\u{1f}') => {
                self.execute(input);
            }

            (State::CsiIgnore, '\u{40}'...'\u{7e}') => {
                self.state = State::Ground;
            }

            // DcsEntry
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

            // DcsParam
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

            // DcsIntermediate
            (State::DcsIntermediate, '\u{20}'...'\u{2f}') => {
                self.collect(input);
            }

            (State::DcsIntermediate, '\u{30}'...'\u{3f}') => {
                self.state = State::DcsIgnore;
            }

            (State::DcsIntermediate, '\u{40}'...'\u{7e}') => {
                self.state = State::DcsPassthrough;
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

            // OscString
            (State::OscString, '\u{07}') => {
                // 0x07 is xterm non-ANSI variant of transition to ground
                self.state = State::Ground;
            }

            (State::OscString, '\u{20}'...'\u{7f}') => {
                self.osc_put(input);
            }

            _ => {}
        }
    }

    fn execute(&mut self, input: char) {
        match input {
            '\u{08}' => self.execute_bs(),
            '\u{09}' => self.execute_ht(),
            '\u{0a}' => self.execute_lf(),
            '\u{0b}' => self.execute_lf(),
            '\u{0c}' => self.execute_lf(),
            '\u{0d}' => self.execute_cr(),
            '\u{0e}' => self.execute_so(),
            '\u{0f}' => self.execute_si(),
            '\u{84}' => self.execute_lf(),
            '\u{85}' => self.execute_nel(),
            '\u{88}' => self.execute_hts(),
            '\u{8d}' => self.execute_ri(),
            _ => {}
        }
    }

    fn print(&self, input: char) {
        // print!("print\n");
    }

    fn collect(&mut self, input: char) {
        self.intermediates.push(input);
    }

    fn esc_dispatch(&self, input: char) {
        // print!("esc_dispatch\n");
    }

    fn param(&mut self, input: char) {
        self.params.push(input);
    }

    fn csi_dispatch(&mut self, input: char) {
        match input {
            '\u{40}' => self.execute_ich(),
            '\u{41}' => self.execute_cuu(),
            '\u{42}' => self.execute_cud(),
            '\u{43}' => self.execute_cuf(),
            '\u{44}' => self.execute_cub(),
            '\u{45}' => self.execute_cnl(),
            '\u{46}' => self.execute_cpl(),
            '\u{47}' => self.execute_cha(),
            '\u{48}' => self.execute_cup(),
            '\u{49}' => self.execute_cht(),
            '\u{4a}' => self.execute_ed(),
            '\u{4b}' => self.execute_el(),
            '\u{4c}' => self.execute_il(),
            '\u{4d}' => self.execute_dl(),
            '\u{50}' => self.execute_dch(),
            '\u{53}' => self.execute_su(),
            '\u{54}' => self.execute_sd(),
            '\u{57}' => self.execute_ctc(),
            '\u{58}' => self.execute_ech(),
            '\u{5a}' => self.execute_cbt(),
            '\u{60}' => self.execute_cha(),
            '\u{61}' => self.execute_cuf(),
            '\u{64}' => self.execute_vpa(),
            '\u{65}' => self.execute_cuu(),
            '\u{66}' => self.execute_cup(),
            '\u{67}' => self.execute_tbc(),
            '\u{68}' => self.execute_sm(),
            '\u{6c}' => self.execute_rm(),
            '\u{6d}' => self.execute_sgr(),
            '\u{70}' => self.execute_decstr(),
            '\u{72}' => self.execute_decstbm(),
            _ => {}
        }
    }

    fn put(&self, _input: char) {}

    fn osc_put(&self, _input: char) {}

    fn clear(&mut self) {
        self.params.clear();
        self.intermediates.clear();
    }

    fn execute_bs(&mut self) {}
    fn execute_ht(&mut self) {}
    fn execute_lf(&mut self) {}
    fn execute_cr(&mut self) {}
    fn execute_so(&mut self) {}
    fn execute_si(&mut self) {}
    fn execute_nel(&mut self) {}
    fn execute_hts(&mut self) {}
    fn execute_ri(&mut self) {}
    fn execute_ich(&mut self) {}
    fn execute_cuu(&mut self) {}
    fn execute_cud(&mut self) {}
    fn execute_cuf(&mut self) {}
    fn execute_cub(&mut self) {}
    fn execute_cnl(&mut self) {}
    fn execute_cpl(&mut self) {}
    fn execute_cha(&mut self) {}
    fn execute_cup(&mut self) {}
    fn execute_cht(&mut self) {}
    fn execute_ed(&mut self) {}
    fn execute_el(&mut self) {}
    fn execute_il(&mut self) {}
    fn execute_dl(&mut self) {}
    fn execute_dch(&mut self) {}
    fn execute_su(&mut self) {}
    fn execute_sd(&mut self) {}
    fn execute_ctc(&mut self) {}
    fn execute_ech(&mut self) {}
    fn execute_cbt(&mut self) {}
    fn execute_vpa(&mut self) {}
    fn execute_tbc(&mut self) {}
    fn execute_sm(&mut self) {}
    fn execute_rm(&mut self) {}
    fn execute_sgr(&mut self) {}
    fn execute_decstr(&mut self) {}
    fn execute_decstbm(&mut self) {}
}