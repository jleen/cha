cat 2of12.txt 2of12inf.txt | sed 's/\r//' | sed 's/%$//' | sort | uniq > words.txt
