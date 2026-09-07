//! The SCE lexicon: known nouns, verbs, adjectives, and proper names.
//!
//! The lexicon is open: unknown words are guessed from position and returned
//! in the `unknown_words` list. Only base lemma forms are stored.

use std::collections::HashSet;
use super::lemma::lemmatize;

fn lemmatize_static(v: &str) -> String { lemmatize(v) }

/// Holds the vocabularies for all open word classes.
#[derive(Debug, Clone)]
pub struct Lexicon {
    nouns: HashSet<String>,
    verbs: HashSet<String>,
    adjectives: HashSet<String>,
    names: HashSet<String>,
    /// Nouns added via DEFAULT_NOUNS (not seeded from corpus). Used to
    /// disambiguate 3sg-s verb forms from plural nouns during parsing.
    core_nouns: HashSet<String>,
    /// Verbs added via DEFAULT_VERBS (not seeded from corpus).
    core_verbs: HashSet<String>,
}

impl Default for Lexicon {
    fn default() -> Self {
        Self::new()
    }
}

impl Lexicon {
    /// Empty lexicon (only function words work out of the box).
    pub fn new() -> Self {
        Lexicon {
            nouns: HashSet::new(),
            verbs: HashSet::new(),
            adjectives: HashSet::new(),
            names: HashSet::new(),
            core_nouns: HashSet::new(),
            core_verbs: HashSet::new(),
        }
    }

    /// Lexicon pre-loaded with common SCE nouns, verbs, and adjectives so that
    /// the SCE.md examples and most corpus sentences parse without a CAN.
    pub fn with_defaults() -> Self {
        let mut lex = Self::new();
        for n in DEFAULT_NOUNS {
            lex.nouns.insert(n.to_lowercase());
            lex.core_nouns.insert(n.to_lowercase());
        }
        for v in DEFAULT_VERBS {
            lex.verbs.insert(v.to_lowercase());
            lex.core_verbs.insert(lemmatize_static(v));
        }
        for a in DEFAULT_ADJS  { lex.add_adjective(a); }
        lex
    }

    pub fn add_noun(&mut self, s: &str) { self.nouns.insert(s.to_lowercase()); }

    /// True if this word is a core noun (from DEFAULT_NOUNS, not seeded from corpus).
    pub fn is_core_noun(&self, s: &str) -> bool { self.core_nouns.contains(&s.to_lowercase()) }

    /// True if the BASE LEMMA is a core verb (from DEFAULT_VERBS).
    pub fn is_core_verb(&self, lemma: &str) -> bool { self.core_verbs.contains(&lemma.to_lowercase()) }
    pub fn add_verb(&mut self, s: &str) { self.verbs.insert(s.to_lowercase()); }
    pub fn add_adjective(&mut self, s: &str) { self.adjectives.insert(s.to_lowercase()); }
    pub fn add_name(&mut self, s: &str) { self.names.insert(s.to_string()); }

    pub fn is_noun(&self, s: &str) -> bool { self.nouns.contains(&s.to_lowercase()) }
    pub fn is_verb(&self, s: &str) -> bool { self.verbs.contains(&s.to_lowercase()) }
    pub fn is_adjective(&self, s: &str) -> bool { self.adjectives.contains(&s.to_lowercase()) }
    pub fn is_name(&self, s: &str) -> bool { self.names.contains(s) }

    pub fn nouns(&self) -> impl Iterator<Item = &str> {
        self.nouns.iter().map(|s| s.as_str())
    }
    pub fn verbs(&self) -> impl Iterator<Item = &str> {
        self.verbs.iter().map(|s| s.as_str())
    }
    pub fn adjectives(&self) -> impl Iterator<Item = &str> {
        self.adjectives.iter().map(|s| s.as_str())
    }
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.names.iter().map(|s| s.as_str())
    }

    /// True if this lowercase word is a function word (closed class).
    pub fn is_function_word(w: &str) -> bool {
        matches!(w,
            "a"|"an"|"the"|"every"|"some"|"no"|"not"|"each"|
            "at"|"least"|"most"|"exactly"|"more"|"unknown"|
            "is"|"are"|"was"|"were"|"am"|"be"|"been"|"being"|
            "has"|"have"|"had"|"having"|
            "do"|"does"|"did"|
            "can"|"cannot"|"could"|"should"|"must"|"may"|"might"|"will"|"would"|"shall"|
            "and"|"or"|"but"|
            "if"|"then"|"there"|"that"|"who"|"which"|"what"|"where"|"when"|"how"|"why"|
            "for"|"as"|"like"|
            "it"|"never"
        )
    }

    /// True if this lowercase word is a preposition.
    pub fn is_prep(w: &str) -> bool {
        matches!(w,
            "to"|"at"|"in"|"on"|"from"|"with"|"for"|"about"|"toward"|"into"|"of"|
            "by"|"as"|"before"|"after"|"than"|"under"|"over"|"above"|"below"|
            "between"|"among"|"through"|"across"|"along"|"around"|"during"|
            "since"|"until"|"up"|"down"|"off"|"out"|"near"|"beside"|"behind"|
            "beyond"|"against"|"within"|"without"|"upon"|"inside"|"outside"|
            "except"|"despite"|"per"|"via"
        )
    }

    /// True if this word could be an adjective/modifier in NP position.
    pub fn could_be_adjective(&self, w: &str) -> bool {
        let lw = w.to_lowercase();
        self.adjectives.contains(&lw)
            || (!self.nouns.contains(&lw)
                && !self.verbs.contains(&lw)
                && !Self::is_function_word(&lw)
                && !Self::is_prep(&lw))
    }
}

static DEFAULT_NOUNS: &[&str] = &[
    "dog","cat","animal","person","customer","card","machine","file","folder","train",
    "box","bag","book","bird","car","bus","ship","plane","bike","vehicle","truck","boat",
    "statement","reason","answer","question","report","claim","identity","account",
    "badge","key","door","car","phone","thing","object","data","task","build",
    "server","order","item","list","step","option","job","bank","manager",
    "employee","admin","guard","number","apple","apples","departure","arrival",
    "thought","input","production","database","man","woman","boy","girl",
    "child","student","teacher","doctor","patient","system","process","result",
    "output","error","bug","test","code","program","function","class","module",
    "package","version","request","response","message","email","call","note",
    "record","entry","field","value","type","name","date","time","day","week",
    "month","year","hour","minute","second","morning","afternoon","evening",
    "night","home","office","room","city","country","world","place","way",
    "side","part","point","end","start","action","plan","goal","policy","rule",
    "condition","case","example","sentence","word","letter","document","page",
    "idea","concept","theory","model","method","approach","solution","problem",
    "issue","topic","subject","project","product","service","feature","tool",
    "resource","access","permission","role","group","team","company","organization",
    "department","business","market","client","vendor","partner","supplier",
    "backup","failure","success","status","state","mode","level","priority",
    "bloop","blip","rerun","retry","promotion","visit","shift","appointment",
    "meeting","conference","session","conversation","discussion","argument",
    "decision","choice","recommendation","suggestion","advice","instruction",
    "direction","command","constraint","limitation","restriction","exception",
    "prototype","draft","telescope","wellbeing","wellbeing","identity","receipt",
    "invoice","contract","agreement","deal","offer","proposal","admission",
    "pizza","money","inbox","queue","buffer","stream","channel","pipeline",
    "endpoint","socket","connection","transaction","operation","query","formula",
    "category","format","syntax","grammar","token","symbol","character","string",
    "text","content","information","knowledge","wisdom","intelligence","skill",
    "ability","power","authority","control","ownership","possession","property",
    "attribute","behavior","reaction","signal","event","incident","situation",
    "environment","context","shelf","change","modification","revision","addition",
    "removal","deletion","insertion","replacement","translation","transformation",
    "mapping","relation","association","connection","link","reference","pointer",
    "variable","constant","parameter","argument","return","array","tree","graph",
    "node","edge","path","route","network","address","host","port","api","uri",
    "appointment","delay","commitment","promise","belief","opinion","view","stance",
    "position","location","direction","movement","speed","rate","amount","count",
    "total","sum","average","minimum","maximum","range","limit","threshold",
    "deadline","duration","period","interval","frequency","chance","probability",
    "risk","benefit","cost","price","payment","specification","design","architecture",
    "interface","protocol","standard","convention","pattern","template","schema",
    "structure","language","help","harm","cure","treatment","diagnosis",
    "certificate","credential","signature","seal","stamp","label","tag","mark",
    "badge","token","ticket","pass","code","key","lock","door","gate","barrier",
    "obstacle","challenge","opportunity","advantage","disadvantage","benefit","cost",
    "tradeoff","compromise","solution","workaround","patch","fix","update","upgrade",
    "library","framework","dependency","component","service","endpoint","api",
    "webhook","callback","handler","middleware","plugin","extension","module",
];

static DEFAULT_VERBS: &[&str] = &[
    "have","possess","contain","belong","need","require","exist","occur","happen",
    "own","feed","enter","accept","reject","know","believe","say","tell","travel","move",
    "go","come","walk","run","fly","drive","ride","swim","climb","fall","rise","grow",
    "think","want","report","like","love","hate","manage","make","leave",
    "arrive","call","see","pet","bark","wait","eat","sleep","precede","process",
    "close","open","save","delete","keep","move","sell","buy","give","take",
    "use","start","stop","fail","try","answer","deny","greet","check","mark",
    "log","allow","admit","disable","retire","decrease","increase","repeat",
    "refuse","update","describe","offer","pay","prefer","perform","ensure",
    "get","add","show","send","discard","archive","print","ping","restart",
    "rerun","touch","correct","misunderstand","miss","hit","win","lose","find",
    "search","seek","ask","explain","define","classify","identify","recognize",
    "detect","monitor","track","measure","calculate","compute","evaluate","analyze",
    "test","verify","validate","approve","confirm","cancel","postpone","delay",
    "schedule","plan","organize","arrange","coordinate","assign","delegate",
    "allocate","distribute","share","publish","announce","notify","alert","warn",
    "inform","document","record","store","retrieve","load","export","import",
    "sync","transfer","copy","paste","cut","insert","replace","remove","clear",
    "reset","initialize","configure","install","uninstall","upgrade","downgrade",
    "restore","deploy","release","rollback","merge","split","divide","combine",
    "join","separate","sort","filter","group","aggregate","transform","convert",
    "encode","decode","encrypt","decrypt","sign","authenticate","authorize",
    "grant","revoke","lock","unlock","protect","expose","hide","toggle","enable",
    "activate","deactivate","suspend","resume","pause","continue","terminate",
    "kill","abort","interrupt","trigger","fire","emit","broadcast","subscribe",
    "unsubscribe","consume","produce","receive","forward","redirect","proxy",
    "cache","render","display","draw","paint","format","parse","tokenize","scan",
    "match","extract","enrich","summarize","rank","score","rate","review","commit",
    "push","pull","fetch","clone","fork","tag","build","compile","run","execute",
    "debug","profile","optimize","refactor","supervise","hire","fire","promote",
    "demote","assess","cover","steal","agree","disagree","negotiate","compete",
    "succeed","achieve","accomplish","finish","begin","initiate","launch",
    "drag","drop","pick","place","position","rotate","flip","mirror","scale",
    "resize","crop","align","attach","detach","connect","disconnect","bind",
    "unbind","associate","relate","correlate","compare","contrast","differentiate",
    "categorize","order","prioritize","weight","normalize","standardize","translate",
    "interpret","generate","synthesize","repair","fix","patch","clean","purge",
    "apologize","help","harm","heal","cure","treat","diagnose","prescribe",
    "recommend","suggest","advise","guide","direct","instruct","teach","learn",
    "study","practice","apply","demonstrate","explore","investigate","research",
    "prove","disprove","support","oppose","challenge","question","doubt","trust",
    "remember","forget","notice","observe","watch","listen","hear","read","write",
    "speak","communicate","express","convey","mean","imply","indicate","signal",
    "declare","claim","assert","state","argue","insist","demand","beg","plead",
    "order","permit","forbid","prevent","block","approach","reach","exceed",
    "surpass","contain","include","exclude","require","depend","inherit","extend",
    "implement","override","return","throw","catch","handle","wrap","unwrap",
    "look-for","knocks-out","apologize-to","pays-more-than","carry","cover",
    "retries","retry","precede","follow","lead","head","picks","places","positions",
    "precedes","follows","leads","apologizes","covers","misunderstands","steals",
    "pets","barks","eats","sleeps","waits","calls","sees","manages","makes",
    "leaves","arrives","reads","writes","speaks","communicates","expresses",
    "conveys","means","implies","indicates","signals","declares","claims","asserts",
    "states","argues","insists","demands","begs","pleads","orders","permits",
    "forbids","prevents","blocks","approaches","reaches","exceeds","surpasses",
    "contains","includes","excludes","requires","depends","inherits","extends",
    "implements","overrides","returns","throws","catches","handles","wraps",
    "unwraps","carries","covers",
];

static DEFAULT_ADJS: &[&str] = &[
    "good","bad","happy","sad","angry","hungry","active","empty","full",
    "fast","slow","red","blue","green","big","small","valid","expired","banned",
    "present","absent","well","true","false","ready","busy","stressful","lazy",
    "important","unhappy","sleepy","hot","cold","smart","bright","clear",
    "open","closed","pending","cancelled","new","old","broken","fixed",
    "possible","available","unavailable","online","offline","public","private",
    "local","remote","internal","external","primary","secondary","main","default",
    "custom","manual","automatic","dynamic","static","simple","complex","easy",
    "hard","difficult","long","short","high","low","large","many","few",
    "recent","latest","earliest","final","initial","original","modified",
    "updated","deleted","created","approved","rejected","completed","failed",
    "successful","running","stopped","paused","idle","free","used","unused",
    "required","optional","mandatory","forbidden","allowed","blocked","enabled",
    "disabled","visible","hidden","known","unknown","unique","duplicate","similar",
    "different","equal","unequal","greater","lesser","rainy","sunny","cloudy",
    "warm","cool","dry","wet","clean","dirty","fresh","stale","safe","unsafe",
    "dangerous","harmless","harmful","useful","useless","helpful","unhelpful",
    "effective","efficient","reliable","stable","secure","insecure","confidential",
    "sensitive","critical","urgent","brown","black","white","gray","grey",
    "yellow","orange","purple","pink","heavy","strong","weak","soft","rough",
    "smooth","sharp","blunt","thin","thick","narrow","wide","deep","shallow",
    "round","square","loud","quiet","noisy","silent","quick","gradual","sudden",
    "early","late","young","mature","experienced","skilled","qualified","certified",
    "annoying","boring","interesting","exciting","fun","dull","entertaining",
    "educational","informative","accurate","inaccurate","correct","incorrect",
    "right","wrong","real","fake","genuine","artificial","natural","organic",
    "synthetic","digital","physical","virtual","abstract","concrete",
    "null","non-null","zero","positive","negative","even","odd","prime","sorted",
    "cached","encrypted","compressed","authenticated","authorized","verified",
    "smarter","bigger","faster","slower","smaller","larger","older","newer",
    "better","worse","cheaper","less-stressful","less-expensive","rainy",
    "inactive","in-progress","on-hold","resolved","unresolved","green",
    "very","extremely","quite","somewhat","fairly","rather","particularly",
    "especially","highly","deeply","truly","fully","completely","partially",
    "mostly","largely","generally","usually","often","sometimes","rarely",
    "never","always","already","still","yet","soon","now","today","tomorrow",
    "yesterday","previously","recently","currently","immediately","eventually",
    "gradually","suddenly","quickly","slowly","carefully","accurately","correctly",
    "properly","appropriately","efficiently","effectively","safely","securely",
    "repeatedly","frequently","occasionally","continuously","constantly",
    "intermittently","periodically","regularly","irregularly","randomly","sequentially",
    "parallel","concurrently","asynchronously","synchronously","automatically",
    "manually","explicitly","implicitly","directly","indirectly","globally","locally",
    "temporarily","permanently","conditionally","unconditionally","recursively",
    "iteratively","lazily","eagerly","strictly","loosely","strongly","weakly",
    "heavily","lightly","broadly","narrowly","deeply","shallowly","widely","closely",
    "own",
];
