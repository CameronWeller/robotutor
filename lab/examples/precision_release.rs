//! Compare decimal measurements without losing reported precision.
type Check<T> = Result<T, String>;
fn require(ok: bool, message: &str) -> Check<()> {
    if ok { Ok(()) } else { Err(message.to_owned()) }
}
// The lexical precision belongs to the reported measurement, even when the numeric
// value is equal. This narrow parser supports positive, finite printed decimals.
#[derive(Debug,PartialEq,Eq)]
struct Reading{digits:String,scale:usize,figures:usize}
impl Reading{
    fn parse(raw:&str)->Check<Self>{
        let (whole,frac)=raw.split_once('.').ok_or("a printed decimal is required")?;
        require(!whole.is_empty()&&!frac.is_empty()&&raw.len()<=20&&whole.bytes().chain(frac.bytes()).all(|b|b.is_ascii_digit()),"unsupported measured decimal")?;
        let digits=format!("{whole}{frac}");let significant=digits.trim_start_matches('0');
        require(!significant.is_empty(),"all-zero measurement is outside this study")?;
        Ok(Self{digits:significant.into(),scale:frac.len(),figures:significant.len()})
    }
    fn value(&self)->(&str,isize){let reduced=self.digits.trim_end_matches('0');(reduced,self.digits.len() as isize-reduced.len() as isize-self.scale as isize)}
}

fn main() -> Check<()> {
    for text in ["2.6", "2.60"] {
        let reading = Reading::parse(text)?;
        println!("{text} reports {} significant figures.", reading.figures);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserve_reported_precision() {
        let short = Reading::parse("2.6").unwrap();
        let precise = Reading::parse("2.60").unwrap();
        assert_eq!(short.value(), precise.value());
        assert_eq!((short.figures, precise.figures), (2, 3));
        assert!(Reading::parse("unavailable").is_err());
    }
}
