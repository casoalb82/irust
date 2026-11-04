use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
struct QuitMessagesFile {
    #[serde(default)]
    messages: Vec<String>,
}

const QUIT_MESSAGES_FILE: &str = "quit_messages.toml";

pub fn load_quit_messages() -> Vec<String> {
    let missing = match crate::storage::data_file_path(QUIT_MESSAGES_FILE) {
        Some(path) => !path.exists(),
        None => true,
    };
    if missing {
        let seeded: Vec<String> = SEED_QUIT_MESSAGES.iter().map(|s| s.to_string()).collect();
        let _ = save_quit_messages(&seeded);
        return seeded;
    }
    crate::storage::load_toml::<QuitMessagesFile>(QUIT_MESSAGES_FILE).messages
}

/// Overwrites the quit-messages file with the current list. Best-effort:
/// a save failure (disk full, permissions...) shouldn't crash the
/// client.
pub fn save_quit_messages(messages: &[String]) -> std::io::Result<()> {
    let file = QuitMessagesFile {
        messages: messages.to_vec(),
    };
    crate::storage::save_toml(QUIT_MESSAGES_FILE, &file, true)
}

pub const SEED_QUIT_MESSAGES: &[&str] = &[
    "irust for president.",
    "irust: a modern client for a old world",
    "irust: a new fragrance for men, by Calvin Klein",
    "irust: anything else would be uncivilized",
    "irust: born to raise hell",
    "irust: causing all sorts of havoc!",
    "irust: cleaner, drier, protects even better",
    "irust: coming soon to a theatre near you!",
    "irust: do not expose to open flame or fire",
    "irust: don't leave home without it!",
    "irust: double your pleasure, double your fun!",
    "irust: EPIC on steroids.",
    "irust: faster than a speeding bullet, more powerful than a locomotive",
    "irust: fit to feel groovy",
    "irust: for distribution only with a new PC",
    "irust: good to the last drop!",
    "irust: it does a body good",
    "irust: it isn't only my script, it's also my client",
    "irust: it keeps going and going and going and going and...",
    "irust: it makes bath-time lots of fun!",
    "irust: it'll get you in, out and on with life.",
    "irust: it's all day strong, all day long",
    "irust: it's everywhere you want to be",
    "irust: it's how steak is done",
    "irust: it's magically delicious!",
    "irust: it's not for the tab-key impaired!",
    "irust: it's not your ordinary stick of gum",
    "irust: it's shagadellic, baby!",
    "irust: it's wax ecstatic",
    "irust: it's what's for lunch",
    "irust: Little. Yellow. Better.",
    "irust: made with real honey.",
    "irust: may cause birth defects",
    "irust: melts in your mouth, not in your hands",
    "irust: more nutritious than a six-pack.",
    "irust: need we say more?",
    "irust: nine out of ten doctors recommend it",
    "irust: nine out of ten gynecologists recommend it",
    "irust: no additives or preservatives",
    "irust: not from concentrate",
    "irust: not to be taken internally",
    "irust: now a major feature film",
    "irust: now in non-drowsy formula too!",
    "irust: now with flavor crystals!",
    "irust: now with Olestra!",
    "irust: now with wings!",
    "irust: now Y2K compatible!",
    "irust: often imitated, never duplicated!",
    "irust: reserve your copy today!",
    "irust: roqz yer nutz!",
    "irust: sanitized for your protection",
    "irust: shaken, not stirred",
    "irust: stays crunchy in milk!",
    "irust: strong enough for a man, pH-balanced for a woman",
    "irust: take the red pill",
    "irust: TASTES GREAT! LESS FILLING! TASTES GREAT! LESS FILLING!",
    "irust: the Cadillac of all clients",
    "irust: the cootiless client",
    "irust: the cure for the common client",
    "irust: the fizzle goes straight to your brain!",
    "irust: the fresh-maker!",
    "irust: the headache medicine",
    "irust: the new hardcore, psycho, nitro client -- in a can",
    "irust: the official IRC client of the 2004 Olympic Games",
    "irust: the ONLY bug-free client",
    "irust: the ONLY three day cure!",
    "irust: the original point-and-click interface.",
    "irust: the OTHER white meat",
    "irust: the quilted quicker picker upper",
    "irust: the right choice for a healthy pet",
    "irust: the un-Cola",
    "irust: to the rescue!",
    "irust: try our low-fat flavor too!",
    "irust: try our Windows Me and Windows XP flavors too!",
    "irust: Tune In, Turn On, Drop Out.",
    "irust: use it, it makes hair grow in funny places!",
    "irust: use it, it makes you bulletproof",
    "irust: use only under adult supervision",
    "irust: you'll have to speak up, I'm wearing a towel",
    "irust: your way, right away",
    "irust: not a flotation device",
    "irust: for external use only",
    "irust: may contain traces of peanuts",
    "irust: contains phenylalanine",
    "irust: antisocial networking",
    "irust by abcX",
    "irust (c) 2026 abcX",
    "irust: just when you thought it couldn't get any better",
    "irust by any other name is still irust",
    "irust has bite! (Just ask Mike Tyson!)",
    "irust Lite I said!",
    "\"got irust?\"",
    "\"I did not have sexual relations with that client, irust.\"",
    "irust is as irust does",
    "*PHEAR MY IRUST-NESS!* BOW TO ME!",
    "*SUBLIMINAL*MESSAGE* USE IRUST *SUBLIMINAL*MESSAGE*",
    "Abort Retry Fail",
    "A friend with weed is a friend indeed",
    "Automatically bored away",
    "Back wit anutha one of doz irust-rockin' beats!",
    "Become an irust Certified Systems Engineer today! Apply within!",
    "Beefcake! BEEFCAKE!",
    "Been around the world and found that only stupid people are breeding.",
    "Bender uses irust. Bite my shiny metal ass!",
    "Bob Barker uses irust. Have your irust spayed or neutered.",
    "Captain Kangaroo uses irust. Shouldn't you?",
    "Check out the sexy irust pr0n at http://127.0.0.1/bx-pron.html",
    "Chester Cheeta uses irust. Ayeuhayueuhayueuh!",
    "Choosey moms choose irust!",
    "Connection reset by abcX?",
    "Did somebody say irust?",
    "Do you... irust?",
    "Does your mommy know you use irust?",
    "Dr. Kavorkian is DYING to use irust. Aren't you?",
    "Dr. Kavorkian would KILL to use irust. Shouldn't you?",
    "Eat, drink and be merry...for tomorrow we die",
    "Elvis has left the building",
    "Everybody was Kung Fu fighting!",
    "For a good time, call 1-900-4irust",
    "Gary Coleman uses irust. Whatchoo talkin bout foo?",
    "Get your free warez from ftp://127.0.0.1!",
    "Grandmaster Flash uses irust, 'coz it's all about money, ain't a damn thing funny",
    "Hanson uses irust. Mommy, can we dress like boys again?",
    "Harry Potter uses irust. Shouldn't you?",
    "Have you hugged your irust today?",
    "He-Man uses irust. *HE HAS THE POOWWEEEEEEEEEERRRRRRRR!!!!!*",
    "Hey, where's the cream filling?!",
    "Hey baby, come sit on my lap. We'll talk about whatever pops up.",
    "Homer Simpson uses irust. D'OH! D'OH! D'OH!",
    "Hrm... I wonder if I paid this month's electr...EOF From client",
    "I came, I saw, I ran away screaming",
    "I got sucked into /dev/null!",
    "I see your irust is as big as mine!",
    "I theenk I need a beeger box!",
    "I wonder what this button marked \"EOF\" does...",
    "I'm off like a prom dress...",
    "I'm out like a light...",
    "If idiots could fly, IRC would be an airport",
    "iT's bEttEr tO bUrN oUt tHaN tO fAdE aWaY",
    "It's game over, man! game over!",
    "It's not TV. It's irust.",
    "Just do it like Nike... BEEATCH!",
    "Leggo my Eggo!",
    "Life is like irust. Ya never know what yer gunna git.",
    "Man I'm *SLEEPY*!!!! My keyboard is slipping away!",
    "Mike Tyson says irust BITES! Do you HEAR what I'm saying?!",
    "mIRCrap: JUST DON'T DO IT!",
    "Mr. Peanut uses irust. Shouldn't you?",
    "Mr. Rogers uses irust. Won't you be my neighbor?",
    "Mr. T uses irust. Shouldn't you, SUCKAH?",
    "Now she knows why her cat stays away from me! DAMNIT!",
    "Occifer, I'm not as think as you stoned I am!",
    "Occifer, take me drunk, I'm home",
    "One irust to rule them all",
    "Outwit. Outplay. Outlast. Survivor irust.",
    "PARTYTIME! 5 seconds to the millenium! 4...3...2...1...EOF From client",
    "Ping-pong timeout",
    "Pretzel Boy uses irust. Shouldn't you?",
    "Reserve your copy of irust for the Amiga today!",
    "Reserve your copy of irust for the Apple Newton today!",
    "Reserve your copy of irust for the Atari 2600 today!",
    "Reserve your copy of irust for the BeOS today!",
    "Reserve your copy of irust for the ColecoVision today!",
    "Reserve your copy of irust for the Commodore 64 today!",
    "Reserve your copy of irust for the Microsoft Xbox today!",
    "Reserve your copy of irust for the Nintendo 64 today!",
    "Reserve your copy of irust for the Nintendo Gameboy today!",
    "Reserve your copy of irust for the Nintendo GameCube today!",
    "Reserve your copy of irust for the PalmPilot today!",
    "Reserve your copy of irust for Windows CE today!",
    "Save water -- drink beer!",
    "Showering in your clothes shows you're crazy. Showering nude shows your nuts.",
    "Silly wabbit, irust is for kids!",
    "Size DOES matter",
    "Tabardation - the inability to master use of the <Tab> key. See: retardation; Headcase.",
    "Terminated.",
    "That's Miss irust to you",
    "That's Mister irust to you",
    "The best part of waking up is irust in your cup",
    "The birds kept calling his name, thought Caw",
    "The Borg use irust. It will be assimilated. Shouldn't you?",
    "The Hobbits use irust. Shouldn't you?",
    "The Invisible Man uses irust, you just can't see it!",
    "The name's X. irust.",
    "The Power Rangers use irust. Shouldn't you?",
    "The Spice Girls use irust (instead of underwear and talent)",
    "They killed Kenny! THOSE BASTARDS!",
    "This irust's for you",
    "Tickle-Me Elmo uses irust. *giggle* *giggle* *giggle*",
    "Tiger Woods uses irust. FORE!",
    "Time to make the donuts",
    "Time wasted: all of it",
    "Tone Loc uses irust, and funky cold medina.",
    "Tony the Tiger uses irust. It's Grrrrrrrrreat!",
    "We are irust of Borg. You will be assimilated. Using ircII is futile.",
    "We are irust of Borg. You will be assimilated. Using mIRC is futile.",
    "We drink more beers than Norm on Cheers!",
    "Went through the desert on an irust with no name.",
    "Where do you want to go today?",
    "Who ate my nuggets?!",
    "Whyismyspacebarnotworking?!",
    "Windows Me, coded entirely by blondes",
    "With an irust here and an irust there, here an irust there an irust everywhere an irust",
    "Wouldn't it be great if the dog could walk itself?",
    "Yo quiero irust",
    "You can breathe without irust, but I wouldn't recommend it",
    "You can /quit any time you like, but you can never leave",
    "THE BEATINGS WILL CONTINUE UNTIL MORALE IMPROVES",
    "If you gaze into the abyss, the abyss gazes also into you",
    "I hear banjos",
    "A gentleman is one who knows how to play the bagpipes, but chooses not to.",
    "All the irust that's fit to print!",
    "Bad irust, bad irust, whatcha gonna do? Whatcha gonna do when they come for you?",
    "bong timeout",
    "Connection reset by beer",
    "Connection reset by pheer",
    "Don't squeeze the irust",
    "Dude! You're getting an irust!",
    "Grab life by the irust",
    "HEY! Must be the irust!",
    "Read error: 1.414213562 (Excessive square root of 2)",
    "Read error: 1.732050808 (Excessive square root of 3)",
    "Read error: 2.71828182846 (Excessive e)",
    "Read error: 2.99792458 x 10^8 meters/second (Excessive speed of light)",
    "Read error: 3.1415926535 (Excessive pi)",
    "Read error: 666 (Connection reset by Satan)",
    "Will the REAL Slim irust please stand up?",
    "Who's your irust?",
    "irust: Better than peer",
    "10 million strong and growing!",
    "Read error: 2004 (10 years of irust isn't nearly enough)",
    "Read error: 420 (Excessive blunts to the dome)",
    "irust: Now available for the Gibson",
    "irust: No wardrobe malfunctions here!",
    "irust: The FAQ maker!",
    "Monkey D. Luffy usa Gomu Gomu no irust!",
    "Roronoa Zoro taglia tutto a fette con lo stile a tre spade irust.",
    "Sanji scalcia a ritmo di Diable Jambe irust.",
    "Nami ruba anche l'ultimo pacchetto di irust.",
    "Usopp racconta balle grandi come irust.",
    "Tony Tony Chopper si trasforma in Monster Point irust.",
    "Nico Robin fa fiorire mille braccia di irust.",
    "Franky urla SUPAAA irust!",
    "Brook ride con le ossa scosse da irust. Yohohoho!",
    "Jinbe nuota nell'oceano di irust.",
    "Trafalgar Law fa ROOM e opera con irust.",
    "Portgas D. Ace brucia tutto con Mera Mera no irust.",
    "Shanks il Rosso naviga verso irust.",
    "Edward Newgate: \"irust e' la mia famiglia\".",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_round_trips_to_an_empty_list() {
        let parsed: QuitMessagesFile = toml::from_str("").unwrap();
        assert!(parsed.messages.is_empty());
    }

    #[test]
    fn file_with_entries_parses_correctly_and_preserves_order() {
        let parsed: QuitMessagesFile = toml::from_str(
            r#"
            messages = ["first one", "second one"]
            "#,
        )
        .unwrap();
        assert_eq!(parsed.messages, vec!["first one".to_string(), "second one".to_string()]);
    }

    #[test]
    fn seed_list_is_non_empty_and_has_no_leftover_bitchx_script_syntax() {
        assert!(
            SEED_QUIT_MESSAGES.len() > 100,
            "expected the full ported BitchX.quit list"
        );
        for m in SEED_QUIT_MESSAGES {
            assert!(!m.contains('$'), "leftover BitchX scripting syntax in: {m:?}");
            assert!(
                !m.to_lowercase().contains("bitchx"),
                "unsubstituted BitchX reference in: {m:?}"
            );
        }
    }

    #[test]
    fn save_and_load_round_trips_through_disk_format() {
        let messages = vec!["custom one".to_string(), "custom two".to_string()];
        let file = QuitMessagesFile {
            messages: messages.clone(),
        };
        let toml_str = toml::to_string_pretty(&file).unwrap();
        let parsed: QuitMessagesFile = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.messages, messages);
    }
}
